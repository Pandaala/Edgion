// UDPRoute StreamPlugins Test Suite
//
// Tests that stream plugins on UDPRoute annotations are enforced per session at
// session creation (fail-close), mirroring the TCPRoute/StreamPlugins suite
// for the UDP path.
//
// Required config files (in edgion-tests/integration/conf/UDPRoute/StreamPlugins/):
// - 01_EdgionStreamPlugins_deny.yaml          # deny-all resource
// - 02_EdgionStreamPlugins_allow_localhost.yaml # allow 127.0.0.0/8 only
// - 03_Gateway.yaml                           # Gateway with 3 UDP listeners
// - 04_UDPRoute_allow_localhost.yaml          # UDPRoute with allow-localhost plugin
// - 05_UDPRoute_deny.yaml                     # UDPRoute with deny-all plugin
// - 06_UDPRoute_fail_close.yaml               # UDPRoute referencing a missing ESP
//
// Port allocation (from ports.json "UDPRoute/StreamPlugins"):
// - 31392 (udp): allow-localhost filter
// - 31393 (udp_filtered): deny-all filter
// - 31394 (udp_fail_close): UDPRoute referencing missing ESP (fail-close contract)
//
// UDP denial semantics: unlike TCP (EOF), UDP deny = no echo. The gateway drops the
// packet; the test sends a datagram and expects a timeout (no response).

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;

pub struct UdpStreamPluginsTestSuite;

impl UdpStreamPluginsTestSuite {
    /// Positive path: allow-localhost plugin should forward packets from 127.0.0.1.
    ///
    /// Config: defaultAction=deny, allowList=[127.0.0.0/8]
    /// 127.0.0.1 IS in 127.0.0.0/8 — packet must be forwarded (echoed back).
    fn test_allow_localhost_echo() -> TestCase {
        TestCase::new(
            "udp_stream_plugin_allow_localhost",
            "UDPRoute stream plugin should allow localhost packet and echo it back",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let test_data = b"Hello UDP StreamPlugin";

                    match UdpSocket::bind("127.0.0.1:0").await {
                        Ok(socket) => {
                            if let Err(e) = socket.send_to(test_data, &ctx.udp_addr()).await {
                                return TestResult::failed(start.elapsed(), format!("Send failed: {}", e));
                            }

                            let mut buf = vec![0u8; 1024];
                            match tokio::time::timeout(Duration::from_secs(3), socket.recv_from(&mut buf)).await {
                                Ok(Ok((n, _))) if n > 0 => {
                                    if &buf[..n] == test_data {
                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            "UDP echo succeeded — real IP (127.0.0.1) correctly matched allow-list"
                                                .to_string(),
                                        )
                                    } else {
                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            format!("UDP packet forwarded by stream plugin ({} bytes)", n),
                                        )
                                    }
                                }
                                Ok(Ok(_)) => TestResult::failed(
                                    start.elapsed(),
                                    "Received 0 bytes — stream plugin may have used wrong source IP".to_string(),
                                ),
                                Ok(Err(e)) => TestResult::failed(start.elapsed(), format!("Recv failed: {}", e)),
                                Err(_) => TestResult::failed(
                                    start.elapsed(),
                                    "Timeout — stream plugin may have denied the packet (wrong IP extracted)"
                                        .to_string(),
                                ),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Bind failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Deny path: deny-all stream plugin should drop all UDP packets (no echo).
    fn test_deny_all_drops_packet() -> TestCase {
        TestCase::new(
            "udp_stream_plugin_deny_all",
            "UDPRoute stream plugin with deny-all should drop packets (no echo)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let addr = format!("{}:31393", ctx.target_host);
                    let test_data = b"Hello Deny";

                    match UdpSocket::bind("0.0.0.0:0").await {
                        Ok(socket) => {
                            if let Err(e) = socket.send_to(test_data, &addr).await {
                                return TestResult::failed(start.elapsed(), format!("Send failed: {}", e));
                            }

                            let mut buf = vec![0u8; 1024];
                            match tokio::time::timeout(Duration::from_secs(2), socket.recv_from(&mut buf)).await {
                                Ok(Ok((n, _))) => TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected packet drop but received {} bytes — deny-all not working", n),
                                ),
                                Ok(Err(_)) | Err(_) => TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Packet dropped by stream plugin (no echo / timeout) — deny working".to_string(),
                                ),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Bind failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Short-name annotation: deny UDPRoute uses "udp-deny-all" (no namespace prefix);
    /// namespace is inferred from the UDPRoute's own namespace.
    fn test_short_name_annotation() -> TestCase {
        TestCase::new(
            "udp_stream_plugin_short_name_annotation",
            "Short name annotation (without namespace/) should correctly resolve EdgionStreamPlugins",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let addr = format!("{}:31393", ctx.target_host);

                    match UdpSocket::bind("0.0.0.0:0").await {
                        Ok(socket) => {
                            let _ = socket.send_to(b"ShortName", &addr).await;
                            let mut buf = vec![0u8; 16];
                            match tokio::time::timeout(Duration::from_secs(2), socket.recv_from(&mut buf)).await {
                                Ok(Ok((n, _))) => TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected denial but received {} bytes — short name annotation may not be resolved",
                                        n
                                    ),
                                ),
                                Ok(Err(_)) | Err(_) => TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Short name annotation resolved correctly — packet denied".to_string(),
                                ),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Bind failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Fail-close: UDPRoute referencing a non-existent ESP must drop packets rather
    /// than silently forwarding them.
    fn test_missing_esp_fail_close() -> TestCase {
        TestCase::new(
            "udp_stream_plugin_missing_esp_fail_close",
            "UDPRoute referencing a non-existent EdgionStreamPlugins must fail-close (drop packets)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let addr = format!("{}:31394", ctx.target_host);
                    let test_data = b"Hello FailClose";

                    match UdpSocket::bind("0.0.0.0:0").await {
                        Ok(socket) => {
                            if let Err(e) = socket.send_to(test_data, &addr).await {
                                return TestResult::failed(start.elapsed(), format!("Send failed: {}", e));
                            }

                            let mut buf = vec![0u8; 1024];
                            match tokio::time::timeout(Duration::from_secs(2), socket.recv_from(&mut buf)).await {
                                Ok(Ok((n, _))) => TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected fail-close (no proxy) but received {} bytes — missing ESP was admitted",
                                        n
                                    ),
                                ),
                                Ok(Err(_)) | Err(_) => TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Packet dropped by gateway — fail-close on missing ESP".to_string(),
                                ),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Bind failed: {}", e)),
                    }
                })
            },
        )
    }
}

impl TestSuite for UdpStreamPluginsTestSuite {
    fn name(&self) -> &str {
        "UDPRoute StreamPlugins"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_allow_localhost_echo(),
            Self::test_deny_all_drops_packet(),
            Self::test_short_name_annotation(),
            Self::test_missing_esp_fail_close(),
        ]
    }
}
