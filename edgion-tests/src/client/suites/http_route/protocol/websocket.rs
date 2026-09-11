// WebSocket Test suite
//
// Required config files (in edgion-tests/integration/conf/):
// - EndpointSlice_edge_test-websocket.yaml    # WebSocket backend service discovery
// - Service_edge_test-websocket.yaml          # WebSocket service definition
// - httproute_default_example-route.yaml      # WebSocket routing rules (Host: test.example.com)
// - Gateway_edge_example-gateway.yaml         # Gateway config
// - GatewayClass__public-gateway.yaml         # GatewayClass config

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use anyhow::{bail, ensure, Context};
use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use std::io::SeekFrom;
use std::time::{Duration, Instant};
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncSeekExt, BufReader};
use tokio::time::{sleep, timeout};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, Message},
};
use uuid::Uuid;

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

    fn test_websocket_conn_est_access_log() -> TestCase {
        TestCase::new(
            "websocket_conn_est_access_log",
            "Connection-established access log arrives while the WebSocket remains usable",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let result = async {
                        // Observe the configured LocalFile output, not the terminal-only
                        // AccessLogStore. Start at EOF before making this request.
                        let mut file = File::open(&ctx.access_log_path)
                            .await
                            .with_context(|| format!("Open configured access log {}", ctx.access_log_path.display()))?;
                        file.seek(SeekFrom::End(0)).await.context("Seek access log to EOF")?;
                        let mut reader = BufReader::new(file).take(1024 * 1024);
                        let trace_id = Uuid::new_v4().simple().to_string();
                        let mut request = ctx.websocket_url().into_client_request()?;
                        if let Some(ref host) = ctx.http_host {
                            request.headers_mut().insert("Host", host.parse()?);
                        }
                        request
                            .headers_mut()
                            .insert("traceparent", format!("00-{trace_id}-0123456789abcdef-00").parse()?);

                        let (mut ws, _) = timeout(Duration::from_secs(5), connect_async(request))
                            .await
                            .context("WebSocket handshake timed out")?
                            .context("WebSocket handshake failed")?;

                        let evidence = timeout(Duration::from_secs(10), async {
                            let mut line = Vec::new();
                            loop {
                                ensure!(reader.limit() > 0, "Access-log scan exceeded 1 MiB without conn_est");
                                let read = reader.read_until(b'\n', &mut line).await.context("Read access log")?;
                                if read == 0 {
                                    sleep(Duration::from_millis(20)).await;
                                    continue;
                                }
                                // A writer may expose a partial record. Retain it until
                                // its newline arrives; the total scan has a hard byte cap.
                                if !line.ends_with(b"\n") {
                                    continue;
                                }
                                if line.windows(trace_id.len()).any(|part| part == trace_id.as_bytes()) {
                                    let entry: serde_json::Value =
                                        serde_json::from_slice(&line).context("Decode this request's access log")?;
                                    if entry["request_info"]["trace_id"]["x-trace-id"].as_str()
                                        == Some(trace_id.as_str())
                                    {
                                        ensure!(
                                            entry["conn_est"].as_bool() == Some(true),
                                            "Access log for this open WebSocket is missing conn_est: true"
                                        );
                                        ensure!(
                                            entry["request_info"]["discover_protocol"].as_str() == Some("websocket"),
                                            "Connection-established log has the wrong protocol"
                                        );
                                        break;
                                    }
                                }
                                line.clear();
                            }

                            // Prove the connection is still usable AFTER seeing the log;
                            // a record emitted only at teardown cannot satisfy this.
                            ws.send(Message::Text("conn-est-still-open".into())).await?;
                            match ws.next().await {
                                Some(Ok(Message::Text(reply))) if reply == "Echo: conn-est-still-open" => Ok(()),
                                _ => bail!("WebSocket echo failed after receiving the connection-established log"),
                            }
                        })
                        .await
                        .context("Timed out waiting for conn_est and a live WebSocket echo");

                        // Always attempt a bounded close, including on assertion failure.
                        // Dropping the socket also releases it if close fails or times out.
                        let close = timeout(Duration::from_secs(2), ws.close(None)).await;
                        drop(ws);
                        evidence??;
                        close
                            .context("WebSocket close timed out")?
                            .context("WebSocket close failed")?;
                        Ok::<(), anyhow::Error>(())
                    }
                    .await;

                    match result {
                        Ok(()) => TestResult::passed(start.elapsed()),
                        Err(error) => TestResult::failed(start.elapsed(), format!("{error:#}")),
                    }
                })
            },
        )
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
            // Kubernetes test-client Pods do not mount the gateway's LocalFile
            // output. Local runs must still fail if their configured log is missing.
            if !matches!(
                std::env::var("EDGION_TEST_K8S_MODE").as_deref(),
                Ok("1" | "true" | "TRUE" | "yes" | "YES")
            ) {
                cases.push(Self::test_websocket_conn_est_access_log());
            }
            cases.push(Self::test_websocket_response_stage_deny_blocks_upgrade());
        }
        cases
    }
}
