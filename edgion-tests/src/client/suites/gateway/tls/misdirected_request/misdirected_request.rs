// HTTP 421 Misdirected Request test suite
//
// Tests that the gateway returns 421 when a request's Host header doesn't match
// the listener that was selected based on the TLS SNI during handshake.
//
// Setup: Three HTTPS listeners share port 31337 on gateway-tls-misdirected:
//   - https-exact:    hostname = sni421-exact.test.com
//   - https-wildcard: hostname = *.sni421.test.com
//   - https-catchall: no hostname (catch-all)
//
// The TLS cert (edge-tls) covers sni421-exact.test.com and *.sni421.test.com via SAN.
//
// Key technique: Each test builds its own reqwest client with resolve() mapping the
// SNI hostname to the target IP. The URL hostname determines TLS SNI, while we
// override the Host header independently to simulate misdirected requests.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs};
use std::time::{Duration, Instant};

const MISDIRECTED_PORT: u16 = 31337;

pub struct MisdirectedRequestTestSuite;

/// Resolve a hostname to an IP address for use with reqwest resolve().
fn resolve_target_ip(target_host: &str) -> IpAddr {
    if target_host.eq_ignore_ascii_case("localhost") {
        return IpAddr::V4(Ipv4Addr::LOCALHOST);
    }
    if let Ok(ip) = target_host.parse::<IpAddr>() {
        return ip;
    }
    (target_host, 0)
        .to_socket_addrs()
        .ok()
        .and_then(|mut addrs| addrs.next())
        .map(|addr| addr.ip())
        .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST))
}

/// Build a reqwest client that resolves `sni_host` to the target gateway IP at the
/// given port. When the client connects to `https://{sni_host}:{port}/...`, the TLS
/// ClientHello will carry `sni_host` as the SNI value, while we can independently
/// set the HTTP Host header on individual requests.
///
/// Forces HTTP/1.1 because HTTP/2 uses the `:authority` pseudo-header (derived from
/// the URL hostname, i.e. the SNI value) which takes precedence over the Host header.
/// In HTTP/1.1 the Host header is respected, allowing SNI != Host testing.
fn build_client_for_sni(target_host: &str, sni_host: &str, port: u16) -> reqwest::Client {
    let ip = resolve_target_ip(target_host);
    let addr = SocketAddr::new(ip, port);
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .danger_accept_invalid_certs(true)
        .no_proxy()
        .http1_only()
        .resolve(sni_host, addr)
        .build()
        .expect("Failed to build reqwest client for SNI test")
}

impl MisdirectedRequestTestSuite {
    // -----------------------------------------------------------------------
    // Case 1: SNI=exact, Host=exact => 200 (same listener)
    // -----------------------------------------------------------------------
    fn test_exact_sni_exact_host_200() -> TestCase {
        TestCase::new(
            "exact_sni_exact_host_200",
            "SNI=sni421-exact.test.com, Host=sni421-exact.test.com => 200 (same exact listener)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let sni = "sni421-exact.test.com";
                    let host = "sni421-exact.test.com";
                    let client = build_client_for_sni(&ctx.target_host, sni, MISDIRECTED_PORT);
                    let url = format!("https://{}:{}/health", sni, MISDIRECTED_PORT);

                    match client.get(&url).header("Host", host).send().await {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Got expected 200 for SNI={}, Host={}", sni, host),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200, got {} for SNI={}, Host={}", status, sni, host),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(
                            start.elapsed(),
                            format!("Request failed for SNI={}, Host={}: {}", sni, host, e),
                        ),
                    }
                })
            },
        )
    }

    // -----------------------------------------------------------------------
    // Case 2: SNI=exact, Host=wildcard => 421 (misdirected)
    // -----------------------------------------------------------------------
    fn test_exact_sni_wildcard_host_421() -> TestCase {
        TestCase::new(
            "exact_sni_wildcard_host_421",
            "SNI=sni421-exact.test.com, Host=foo.sni421.test.com => 421 (SNI->exact, Host->wildcard)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let sni = "sni421-exact.test.com";
                    let host = "foo.sni421.test.com";
                    let client = build_client_for_sni(&ctx.target_host, sni, MISDIRECTED_PORT);
                    let url = format!("https://{}:{}/health", sni, MISDIRECTED_PORT);

                    match client.get(&url).header("Host", host).send().await {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status == 421 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Got expected 421 for SNI={}, Host={}", sni, host),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 421, got {} for SNI={}, Host={}", status, sni, host),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(
                            start.elapsed(),
                            format!("Request failed for SNI={}, Host={}: {}", sni, host, e),
                        ),
                    }
                })
            },
        )
    }

    // -----------------------------------------------------------------------
    // Case 3: SNI=wildcard, Host=wildcard => 200 (same listener)
    // -----------------------------------------------------------------------
    fn test_wildcard_sni_wildcard_host_200() -> TestCase {
        TestCase::new(
            "wildcard_sni_wildcard_host_200",
            "SNI=foo.sni421.test.com, Host=foo.sni421.test.com => 200 (same wildcard listener)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let sni = "foo.sni421.test.com";
                    let host = "foo.sni421.test.com";
                    let client = build_client_for_sni(&ctx.target_host, sni, MISDIRECTED_PORT);
                    let url = format!("https://{}:{}/health", sni, MISDIRECTED_PORT);

                    match client.get(&url).header("Host", host).send().await {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Got expected 200 for SNI={}, Host={}", sni, host),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200, got {} for SNI={}, Host={}", status, sni, host),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(
                            start.elapsed(),
                            format!("Request failed for SNI={}, Host={}: {}", sni, host, e),
                        ),
                    }
                })
            },
        )
    }

    // -----------------------------------------------------------------------
    // Case 4: SNI=unknown, Host=unknown => 200 (both hit catch-all, catch-all route matches)
    // -----------------------------------------------------------------------
    fn test_catchall_sni_catchall_host_200() -> TestCase {
        TestCase::new(
            "catchall_sni_catchall_host_200",
            "SNI=unknown.test.com, Host=unknown.test.com => 200 (both->catch-all, no 421, catch-all route serves)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let sni = "unknown.test.com";
                    let host = "unknown.test.com";
                    let client = build_client_for_sni(&ctx.target_host, sni, MISDIRECTED_PORT);
                    let url = format!("https://{}:{}/health", sni, MISDIRECTED_PORT);

                    match client.get(&url).header("Host", host).send().await {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!(
                                        "Got expected 200 for SNI={}, Host={} (both catch-all, no mismatch)",
                                        sni, host
                                    ),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200, got {} for SNI={}, Host={}", status, sni, host),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(
                            start.elapsed(),
                            format!("Request failed for SNI={}, Host={}: {}", sni, host, e),
                        ),
                    }
                })
            },
        )
    }

    // -----------------------------------------------------------------------
    // Case 5: SNI=unknown, Host=exact => 421 (SNI->catch-all, Host->exact)
    // -----------------------------------------------------------------------
    fn test_catchall_sni_exact_host_421() -> TestCase {
        TestCase::new(
            "catchall_sni_exact_host_421",
            "SNI=unknown.test.com, Host=sni421-exact.test.com => 421 (SNI->catch-all, Host->exact listener)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let sni = "unknown.test.com";
                    let host = "sni421-exact.test.com";
                    let client = build_client_for_sni(&ctx.target_host, sni, MISDIRECTED_PORT);
                    let url = format!("https://{}:{}/health", sni, MISDIRECTED_PORT);

                    match client.get(&url).header("Host", host).send().await {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status == 421 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Got expected 421 for SNI={}, Host={}", sni, host),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 421, got {} for SNI={}, Host={}", status, sni, host),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(
                            start.elapsed(),
                            format!("Request failed for SNI={}, Host={}: {}", sni, host, e),
                        ),
                    }
                })
            },
        )
    }
    // -----------------------------------------------------------------------
    // Case 6: SNI=wildcard, Host=exact => 421 (SNI->wildcard, Host->exact)
    // -----------------------------------------------------------------------
    fn test_wildcard_sni_exact_host_421() -> TestCase {
        TestCase::new(
            "wildcard_sni_exact_host_421",
            "SNI=foo.sni421.test.com, Host=sni421-exact.test.com => 421 (SNI->wildcard, Host->exact listener)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let sni = "foo.sni421.test.com";
                    let host = "sni421-exact.test.com";
                    let client = build_client_for_sni(&ctx.target_host, sni, MISDIRECTED_PORT);
                    let url = format!("https://{}:{}/health", sni, MISDIRECTED_PORT);

                    match client.get(&url).header("Host", host).send().await {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status == 421 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Got expected 421 for SNI={}, Host={}", sni, host),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 421, got {} for SNI={}, Host={}", status, sni, host),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(
                            start.elapsed(),
                            format!("Request failed for SNI={}, Host={}: {}", sni, host, e),
                        ),
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for MisdirectedRequestTestSuite {
    fn name(&self) -> &str {
        "MisdirectedRequest"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_exact_sni_exact_host_200(),
            Self::test_exact_sni_wildcard_host_421(),
            Self::test_wildcard_sni_wildcard_host_200(),
            Self::test_catchall_sni_catchall_host_200(),
            Self::test_catchall_sni_exact_host_421(),
            Self::test_wildcard_sni_exact_host_421(),
        ]
    }
}
