// DnsResolution test suite
//
// End-to-end coverage of ExternalName resolution modes, driven hermetically via
// the test_server's fake HTTP DNS endpoint (`/dns/resolve`, `/dns/hits/{host}`).
// No real DNS is involved.
//
// Scenarios (all resolve to the local test backend 127.0.0.1:30001):
//   1. system       — `dns-resolution-mode: system`, OS resolver per request,
//      externalName=localhost.
//   2. httpdns      — default gateway mode, `dns-resolver-ref` → httpdns
//      LinkSys. The Gateway endpoint table resolves through the httpdns
//      endpoint in the background and the resolved backend serves.
//   3. gateway      — gateway mode with the httpdns resolver: consecutive
//      requests trigger no resolution (the table, not requests, resolves).
//   4. httpdns-fallback — httpdns endpoint DOWN + `fallback: system`: the table
//      falls back to the OS resolver and still serves.
//   5. controller   — `dns-resolution-mode: controller`: the Controller resolves
//      and publishes in-memory EndpointSlices through conf_sync; works in this
//      FileSystem-mode harness as in Kubernetes.
//   6. L4 (TCP) in every mode — `dns-l4-{gateway,system,controller}` behind one
//      TCP listener each, echoing through the local TCP backend (30010).
//   7. L4 (UDP) system mode — `dns-l4-system` behind a UDP listener: a new
//      session resolves through the OS resolver and echoes via 30011.
//
// Required config (edgion-tests/integration/conf/HTTPRoute/Backend/DnsResolution/):
//   - 00_LinkSys_*_dns-httpdns-local        # httpdns → /dns/resolve, returns 127.0.0.1
//   - 00_LinkSys_*_dns-httpdns-dead         # httpdns → down endpoint, fallback: system
//   - 01_Service_*_dns-{system,gateway-cached,httpdns,httpdns-fallback,controller}
//   - 01_Service_*_dns-l4-{gateway,system,controller}
//   - 02_Gateway_edgion-test_dns-res-gateway  # HTTP 31360; TCP 31362-31364; UDP 31365
//   - 03_HTTPRoute_*                          # hostname → backend Service
//   - 03_{TCP,UDP}Route_*_dns-l4-*            # L4 listener → backend Service
// The L4 listeners map onto ports.json fields: `tcp` = gateway mode,
// `tcp_filtered` = system mode, `tcp_bare` = controller mode, `udp` = system mode.
// Requires securityProtect.allowLoopbackUpstream=true (dedicated EdgionGatewayConfig).

use crate::framework::{env_host_or, TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};

const SUITE_PORT_KEY: &str = "HTTPRoute/Backend/DnsResolution";
const L4_IO_TIMEOUT: Duration = Duration::from_secs(5);

pub struct DnsResolutionTestSuite;

impl DnsResolutionTestSuite {
    fn dns_server_base() -> String {
        let host = env_host_or("EDGION_TEST_DNS_SERVER_HOST", "127.0.0.1");
        format!("http://{host}:30001")
    }

    /// Read the httpdns resolve hit-count for `host` from the test server.
    async fn dns_hits(client: &reqwest::Client, host: &str) -> Result<u64, String> {
        let url = format!("{}/dns/hits/{host}", Self::dns_server_base());
        let resp = client
            .get(&url)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
            .map_err(|e| format!("GET {url} failed: {e}"))?;
        let json: serde_json::Value = resp.json().await.map_err(|e| format!("parse hits json failed: {e}"))?;
        json.get("hits")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| format!("missing hits in {json}"))
    }

    /// Send one request to the gateway with the given Host and return the status.
    async fn get_status(ctx: &TestContext, host: &str) -> Result<u16, String> {
        let url = format!("{}/echo", ctx.http_url());
        let resp = ctx
            .http_client
            .get(&url)
            .timeout(std::time::Duration::from_secs(10))
            .header("Host", host)
            .send()
            .await
            .map_err(|e| format!("request to {host} failed: {e}"))?;
        Ok(resp.status().as_u16())
    }

    /// system mode resolves via the OS resolver per request (externalName=localhost)
    /// and serves from the local backend.
    fn test_system_mode_resolves_and_serves() -> TestCase {
        TestCase::new(
            "system_mode_resolves_and_serves",
            "system-mode ExternalName resolves (OS resolver) and serves 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match Self::get_status(&ctx, "system.dns.test").await {
                        Ok(200) => TestResult::passed_with_message(
                            start.elapsed(),
                            "system-mode resolution reached the backend (200)".to_string(),
                        ),
                        Ok(other) => TestResult::failed(start.elapsed(), format!("expected 200, got {other}")),
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// httpdns resolves through the fake HTTP DNS endpoint (hit count > 0) via
    /// the Gateway endpoint table and serves.
    fn test_httpdns_resolves_and_serves() -> TestCase {
        TestCase::new(
            "httpdns_resolves_and_serves",
            "httpdns resolver feeds the endpoint table and serves 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let host = "httpdns.dns.test";
                    for _ in 0..2 {
                        if let Err(e) = Self::expect_ok(&ctx, host).await {
                            return TestResult::failed(start.elapsed(), e);
                        }
                    }
                    match Self::dns_hits(&ctx.http_client, host).await {
                        Ok(hits) if hits > 0 => TestResult::passed_with_message(
                            start.elapsed(),
                            format!("served through httpdns-resolved endpoints (hits={hits})"),
                        ),
                        Ok(_) => TestResult::failed(start.elapsed(), "200 without any httpdns resolution".to_string()),
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// gateway mode resolves in the background: requests right after the first
    /// trigger no httpdns resolution (at most one background refresh lands).
    fn test_gateway_mode_requests_do_not_resolve() -> TestCase {
        TestCase::new(
            "gateway_mode_requests_do_not_resolve",
            "gateway mode serves consecutive requests from the endpoint table (no per-request resolve)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    // Route hostname (Host header) and the resolver hit-count key differ:
                    // the httpdns endpoint counts by the `{domain}` it resolves, which the
                    // gateway fills with the Service's externalName, NOT the route hostname.
                    let route_host = "gateway-cached.dns.test";
                    let resolver_key = "gwcached.dns.test"; // == Service.spec.externalName

                    // The table has resolved at least once before the first request
                    // is served (bounded first-answer wait) → hit count >= 1.
                    if let Err(e) = Self::expect_ok(&ctx, route_host).await {
                        return TestResult::failed(start.elapsed(), e);
                    }
                    let hits_after_first = match Self::dns_hits(&ctx.http_client, resolver_key).await {
                        Ok(n) => n,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    // Guard against a vacuous 0 == 0 pass: gateway mode must have resolved
                    // at least once through the httpdns endpoint.
                    if hits_after_first < 1 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("expected the first request to resolve via httpdns, hits={hits_after_first}"),
                        );
                    }

                    // Several more requests immediately after. A per-request resolver
                    // would add one hit each; the endpoint table adds at most one
                    // background refresh (TTL-less httpdns refreshes every ~5 s).
                    const FOLLOW_UPS: u64 = 5;
                    for _ in 0..FOLLOW_UPS {
                        if let Err(e) = Self::expect_ok(&ctx, route_host).await {
                            return TestResult::failed(start.elapsed(), e);
                        }
                    }
                    let hits_after_follow_ups = match Self::dns_hits(&ctx.http_client, resolver_key).await {
                        Ok(n) => n,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    let grown = hits_after_follow_ups.saturating_sub(hits_after_first);
                    if grown <= 1 {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            format!("{FOLLOW_UPS} requests triggered no per-request resolution (hits {hits_after_first} -> {hits_after_follow_ups})"),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "expected no per-request resolution, hits went {hits_after_first} -> {hits_after_follow_ups} over {FOLLOW_UPS} requests"
                            ),
                        )
                    }
                })
            },
        )
    }

    /// httpdns endpoint DOWN + fallback: system → still resolves and serves.
    fn test_httpdns_fallback_when_endpoint_down() -> TestCase {
        TestCase::new(
            "httpdns_fallback_when_endpoint_down",
            "httpdns endpoint down: the endpoint table falls back to the OS resolver and serves 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match Self::get_status(&ctx, "httpdns-fallback.dns.test").await {
                        Ok(200) => TestResult::passed_with_message(
                            start.elapsed(),
                            "system fallback resolved the ExternalName and served (200)".to_string(),
                        ),
                        Ok(other) => {
                            TestResult::failed(start.elapsed(), format!("expected 200 via fallback, got {other}"))
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// controller mode: the Controller resolves and publishes in-memory
    /// EndpointSlices; the Gateway serves through them.
    fn test_controller_mode_publishes_endpoints() -> TestCase {
        TestCase::new(
            "controller_mode_publishes_endpoints",
            "controller-mode ExternalName is served through Controller-published endpoints",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    match Self::expect_ok(&ctx, "controller.dns.test").await {
                        Ok(()) => TestResult::passed_with_message(
                            start.elapsed(),
                            "Controller-published endpoints served (200)".to_string(),
                        ),
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Send one payload over a TCP connection to `addr` and require the echo.
    async fn tcp_echo(addr: &str, payload: &[u8]) -> Result<(), String> {
        let exchange = async {
            let mut stream = TcpStream::connect(addr)
                .await
                .map_err(|e| format!("connect {addr} failed: {e}"))?;
            stream
                .write_all(payload)
                .await
                .map_err(|e| format!("write to {addr} failed: {e}"))?;
            let mut echoed = vec![0u8; payload.len()];
            stream
                .read_exact(&mut echoed)
                .await
                .map_err(|e| format!("read from {addr} failed: {e}"))?;
            if echoed == payload {
                Ok(())
            } else {
                Err(format!("echo mismatch from {addr}"))
            }
        };
        tokio::time::timeout(L4_IO_TIMEOUT, exchange)
            .await
            .map_err(|_| format!("TCP echo via {addr} timed out"))?
    }

    fn tcp_mode_case(
        name: &'static str,
        mode: &'static str,
        addr: fn(&TestContext) -> Result<String, String>,
    ) -> TestCase {
        TestCase::new(
            name,
            "TCPRoute to an ExternalName Service echoes through the resolved backend",
            move |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let addr = match addr(&ctx) {
                        Ok(addr) => addr,
                        Err(e) => return TestResult::failed(start.elapsed(), e),
                    };
                    // Two connections: the second must not depend on state the
                    // first connection created.
                    for attempt in 0..2 {
                        let payload = format!("dns-l4-{mode}-{attempt}");
                        if let Err(e) = Self::tcp_echo(&addr, payload.as_bytes()).await {
                            return TestResult::failed(start.elapsed(), format!("{mode} mode: {e}"));
                        }
                    }
                    TestResult::passed_with_message(
                        start.elapsed(),
                        format!("{mode}-mode ExternalName served TCP echo via {addr}"),
                    )
                })
            },
        )
    }

    fn test_tcp_gateway_mode_echo() -> TestCase {
        Self::tcp_mode_case("tcp_gateway_mode_echo", "gateway", |ctx| Ok(ctx.tcp_addr()))
    }

    fn test_tcp_system_mode_echo() -> TestCase {
        Self::tcp_mode_case("tcp_system_mode_echo", "system", |ctx| Ok(ctx.tcp_filtered_addr()))
    }

    fn test_tcp_controller_mode_echo() -> TestCase {
        Self::tcp_mode_case("tcp_controller_mode_echo", "controller", |ctx| {
            let port = crate::port_config::PortConfig::load()?
                .get_ports(SUITE_PORT_KEY)?
                .tcp_bare
                .ok_or_else(|| format!("ports.json {SUITE_PORT_KEY}.tcp_bare is not set"))?;
            Ok(format!("{}:{port}", ctx.target_host))
        })
    }

    /// system mode over UDP: the first datagram of a session resolves through the
    /// OS resolver within a short budget, so a lost first datagram is retried.
    fn test_udp_system_mode_echo() -> TestCase {
        TestCase::new(
            "udp_system_mode_echo",
            "UDPRoute to a system-mode ExternalName Service echoes through the resolved backend",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let addr = ctx.udp_addr();
                    let socket = match UdpSocket::bind("0.0.0.0:0").await {
                        Ok(socket) => socket,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("bind failed: {e}")),
                    };
                    let payload = b"dns-l4-system-udp";
                    let mut last_error = String::new();
                    for _ in 0..3 {
                        if let Err(e) = socket.send_to(payload, &addr).await {
                            return TestResult::failed(start.elapsed(), format!("send to {addr} failed: {e}"));
                        }
                        let mut buffer = [0u8; 256];
                        match tokio::time::timeout(Duration::from_secs(2), socket.recv_from(&mut buffer)).await {
                            Ok(Ok((n, _))) if &buffer[..n] == payload => {
                                return TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("system-mode ExternalName served UDP echo via {addr}"),
                                );
                            }
                            Ok(Ok(_)) => last_error = "echo mismatch".to_string(),
                            Ok(Err(e)) => last_error = format!("recv failed: {e}"),
                            Err(_) => last_error = "timed out waiting for the echo".to_string(),
                        }
                    }
                    TestResult::failed(start.elapsed(), format!("UDP via {addr}: {last_error}"))
                })
            },
        )
    }

    async fn expect_ok(ctx: &TestContext, host: &str) -> Result<(), String> {
        match Self::get_status(ctx, host).await? {
            200 => Ok(()),
            other => Err(format!("expected 200 for {host}, got {other}")),
        }
    }
}

#[async_trait]
impl TestSuite for DnsResolutionTestSuite {
    fn name(&self) -> &str {
        "DnsResolution"
    }

    fn port_key(&self) -> Option<&str> {
        Some("HTTPRoute/Backend/DnsResolution")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_system_mode_resolves_and_serves(),
            Self::test_httpdns_resolves_and_serves(),
            Self::test_gateway_mode_requests_do_not_resolve(),
            Self::test_httpdns_fallback_when_endpoint_down(),
            Self::test_controller_mode_publishes_endpoints(),
            Self::test_tcp_gateway_mode_echo(),
            Self::test_tcp_system_mode_echo(),
            Self::test_tcp_controller_mode_echo(),
            Self::test_udp_system_mode_echo(),
        ]
    }
}
