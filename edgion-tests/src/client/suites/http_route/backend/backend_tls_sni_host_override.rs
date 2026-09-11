// ============================================================================
// BackendTLS SNI Host Override Integration Test Suite
// ============================================================================
//
// Verifies that the Gateway sends the correct TLS SNI to the upstream backend
// as dictated by BackendTLSPolicy.spec.validation.hostname.
//
// Setup:
//   - A single backend server on port 30026 uses a per-SNI cert resolver:
//       SNI = "hostname-a.example.com"  ->  cert A (SAN: hostname-a.example.com)
//       SNI = "hostname-b.example.com"  ->  cert B (SAN: hostname-b.example.com)
//       any other SNI / no SNI          ->  handshake rejected (unrecognized_name)
//   - Two Services (btls-sni-service-a, btls-sni-service-b) both point to 30026.
//   - Two BackendTLSPolicy objects set different validation.hostname values.
//   - Two HTTPRoutes (host-a.example.com, host-b.example.com) route to the
//     respective services.
//
// The only way both routes return 200 is if the Gateway correctly sends the
// per-policy hostname as the TLS SNI — which is exactly what we are testing.
//
// Test cases:
//   1. test_sni_a_resolves_correct_cert  — host-a -> policy A -> SNI=hostname-a -> 200
//   2. test_sni_b_resolves_correct_cert  — host-b -> policy B -> SNI=hostname-b -> 200
//   3. test_concurrent_a_b_dispatch      — interleaved requests; all 200, proving
//                                          no SNI confusion across concurrent connections

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;

pub struct BackendTlsSniHostOverrideTestSuite;

impl BackendTlsSniHostOverrideTestSuite {
    /// Send a request with Host: host-a.example.com and assert 200.
    ///
    /// The Gateway must present SNI=hostname-a.example.com to the backend.
    /// The multi-SNI resolver returns cert A for that SNI; the SAN on cert A
    /// matches the BackendTLSPolicy hostname, so the TLS handshake succeeds.
    fn test_sni_a_resolves_correct_cert() -> TestCase {
        TestCase::new(
            "sni_a_resolves_correct_cert",
            "Host: host-a.example.com -> policy A -> Gateway sends SNI hostname-a.example.com -> 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let client = reqwest::Client::builder()
                        .connect_timeout(std::time::Duration::from_secs(5))
                        .timeout(std::time::Duration::from_secs(10))
                        .no_proxy()
                        .redirect(reqwest::redirect::Policy::none())
                        .build()
                        .expect("failed to build HTTP client");

                    let url = format!("http://{}:{}/echo", ctx.target_host, ctx.http_port);

                    let resp = match client
                        .get(&url)
                        .header("host", "host-a.example.com")
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker("btls-sni-a"),
                        )
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Request with host-a.example.com failed: {}", e),
                            );
                        }
                    };

                    let status = resp.status().as_u16();
                    if status != 200 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected 200 for host-a.example.com (SNI=hostname-a.example.com), \
                                 got {}. This means the Gateway did not send the correct SNI or \
                                 the CA/SAN validation failed.",
                                status
                            ),
                        );
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        "host-a.example.com -> 200 (Gateway correctly sent SNI=hostname-a.example.com)".to_string(),
                    )
                })
            },
        )
    }

    /// Send a request with Host: host-b.example.com and assert 200.
    ///
    /// The Gateway must present SNI=hostname-b.example.com to the backend.
    fn test_sni_b_resolves_correct_cert() -> TestCase {
        TestCase::new(
            "sni_b_resolves_correct_cert",
            "Host: host-b.example.com -> policy B -> Gateway sends SNI hostname-b.example.com -> 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let client = reqwest::Client::builder()
                        .connect_timeout(std::time::Duration::from_secs(5))
                        .timeout(std::time::Duration::from_secs(10))
                        .no_proxy()
                        .redirect(reqwest::redirect::Policy::none())
                        .build()
                        .expect("failed to build HTTP client");

                    let url = format!("http://{}:{}/echo", ctx.target_host, ctx.http_port);

                    let resp = match client
                        .get(&url)
                        .header("host", "host-b.example.com")
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker("btls-sni-b"),
                        )
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Request with host-b.example.com failed: {}", e),
                            );
                        }
                    };

                    let status = resp.status().as_u16();
                    if status != 200 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected 200 for host-b.example.com (SNI=hostname-b.example.com), \
                                 got {}. This means the Gateway did not send the correct SNI or \
                                 the CA/SAN validation failed.",
                                status
                            ),
                        );
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        "host-b.example.com -> 200 (Gateway correctly sent SNI=hostname-b.example.com)".to_string(),
                    )
                })
            },
        )
    }

    /// Interleave 4 host-a and 4 host-b requests and assert all 8 return 200.
    ///
    /// This guards against SNI confusion that could arise if the Gateway
    /// reuses a TLS connection established for one policy when serving a
    /// request that belongs to a different policy (connection pooling hazard).
    fn test_concurrent_a_b_dispatch() -> TestCase {
        TestCase::new(
            "concurrent_a_b_dispatch",
            "Interleaved 4x host-a + 4x host-b requests all return 200 (no SNI confusion)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let client = reqwest::Client::builder()
                        .connect_timeout(std::time::Duration::from_secs(5))
                        .timeout(std::time::Duration::from_secs(10))
                        .no_proxy()
                        .redirect(reqwest::redirect::Policy::none())
                        .build()
                        .expect("failed to build HTTP client");

                    let url = format!("http://{}:{}/echo", ctx.target_host, ctx.http_port);

                    // Interleave requests: A, B, A, B, A, B, A, B
                    for i in 0u32..4 {
                        for (host, label) in [("host-a.example.com", "a"), ("host-b.example.com", "b")] {
                            let trace_id = format!("btls-sni-concurrent-{}-{}", label, i);
                            let resp = match client
                                .get(&url)
                                .header("host", host)
                                .header(
                                    "traceparent",
                                    crate::access_log_client::traceparent_for_marker(&trace_id),
                                )
                                .send()
                                .await
                            {
                                Ok(r) => r,
                                Err(e) => {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!("Request {} (host={}) failed: {}", trace_id, host, e),
                                    );
                                }
                            };

                            let status = resp.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Request {} (host={}): expected 200, got {}. \
                                         Possible SNI mismatch or connection-reuse with wrong SNI.",
                                        trace_id, host, status
                                    ),
                                );
                            }
                        }
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        "All 8 interleaved requests (4x host-a + 4x host-b) returned 200; \
                         no SNI confusion detected."
                            .to_string(),
                    )
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for BackendTlsSniHostOverrideTestSuite {
    fn name(&self) -> &str {
        "BackendTLS SNI Host Override"
    }

    fn port_key(&self) -> Option<&str> {
        Some("HTTPRoute/Backend/BackendTLSSNIHostOverride")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_sni_a_resolves_correct_cert(),
            Self::test_sni_b_resolves_correct_cert(),
            Self::test_concurrent_a_b_dispatch(),
        ]
    }
}
