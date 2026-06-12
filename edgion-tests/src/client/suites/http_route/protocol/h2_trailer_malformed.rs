use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::{Duration, Instant};

pub struct H2TrailerMalformedTestSuite;

impl H2TrailerMalformedTestSuite {
    /// Verify that the Gateway returns a deterministic error (5xx or H2 stream error)
    /// when the upstream backend produces a protocol-violating H2 stream, and does
    /// NOT hang or panic.
    fn test_gateway_returns_error_not_panic() -> TestCase {
        TestCase::new(
            "gateway_returns_error_not_panic",
            "Gateway returns 5xx or H2 stream error for upstream H2 protocol violation, no hang",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let client = reqwest::Client::builder()
                        .http2_prior_knowledge()
                        .no_proxy()
                        .timeout(Duration::from_secs(5))
                        .build()
                        .unwrap();

                    let url = ctx.http_url();

                    match client.get(&url).send().await {
                        Ok(resp) => {
                            let status = resp.status();
                            // The Gateway must surface the upstream failure as a 5xx.
                            // A 200 would mean the backend appeared healthy — unexpected.
                            if status.is_server_error() {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Gateway correctly returned {} for upstream H2 violation", status),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 5xx from gateway for upstream H2 violation, got {}", status),
                                )
                            }
                        }
                        // reqwest may surface the upstream RST_STREAM as an H2 stream error
                        // before a complete HTTP response is assembled.  That is also an
                        // acceptable outcome — it proves the Gateway did not hang or panic,
                        // and it forwarded the error to the downstream client.
                        Err(ref e) if e.is_timeout() => TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Request timed out waiting for gateway response — gateway may be hanging: {}",
                                e
                            ),
                        ),
                        Err(e) => TestResult::passed_with_message(
                            start.elapsed(),
                            format!(
                                "Gateway returned H2/IO error (acceptable: upstream RST propagated): {}",
                                e
                            ),
                        ),
                    }
                })
            },
        )
    }

    /// Send the same malformed-upstream request 5 times in a row, then verify the
    /// Gateway is still alive by checking it accepts a new TCP connection.
    /// A crashed Gateway process would refuse new connections entirely.
    fn test_gateway_alive_after_repeated_failures() -> TestCase {
        TestCase::new(
            "gateway_alive_after_repeated_failures",
            "Gateway remains alive and accepts new connections after repeated upstream H2 violations",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let client = reqwest::Client::builder()
                        .http2_prior_knowledge()
                        .no_proxy()
                        .timeout(Duration::from_secs(5))
                        .build()
                        .unwrap();

                    let url = ctx.http_url();

                    // Fire 5 requests that will all hit the malformed upstream.
                    for i in 1..=5u8 {
                        let _ = client.get(&url).send().await;
                        // Small yield to allow Gateway to process errors between requests.
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        println!("H2TrailerMalformed: round {i}/5 done");
                    }

                    // Gateway liveness check: attempt a fresh TCP connection to the
                    // Gateway listener port.  If the process has crashed, this will fail
                    // with "connection refused".  We do not need a valid HTTP response —
                    // just a successful TCP handshake proves the process is still running.
                    let addr = format!("{}:{}", ctx.target_host, ctx.http_port);
                    match tokio::time::timeout(Duration::from_secs(3), tokio::net::TcpStream::connect(&addr)).await {
                        Ok(Ok(_)) => TestResult::passed_with_message(
                            start.elapsed(),
                            format!(
                                "Gateway still alive after 5 upstream H2 violations (TCP connect to {} succeeded)",
                                addr
                            ),
                        ),
                        Ok(Err(e)) => TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Gateway appears dead after repeated H2 violations — TCP connect to {} failed: {}",
                                addr, e
                            ),
                        ),
                        Err(_) => TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Gateway liveness TCP connect to {} timed out — process may be stuck",
                                addr
                            ),
                        ),
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for H2TrailerMalformedTestSuite {
    fn name(&self) -> &str {
        "H2TrailerMalformed"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_gateway_returns_error_not_panic(),
            Self::test_gateway_alive_after_repeated_failures(),
        ]
    }
}
