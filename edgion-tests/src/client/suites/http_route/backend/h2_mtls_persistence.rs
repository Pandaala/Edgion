// ============================================================================
// H2 Backend mTLS + SessionPersistence Integration Test Suite
// ============================================================================
//
// Verifies the combination of three capabilities working end-to-end:
//
//   1. Client -> Gateway: plain HTTP (listener port 31384)
//   2. Gateway -> backend: HTTP/2 over TLS (ALPN h2) + mTLS (client cert)
//      The test_server mTLS backend on port 30052 advertises ALPN ["h2", "http/1.1"],
//      so Pingora negotiates h2 automatically when BackendTLSPolicy is applied.
//   3. SessionPersistence Cookie: Gateway sets "edgion-h2-mtls-sp" cookie on the
//      first request; subsequent requests carrying the cookie route to the same
//      backend endpoint (stable routing).
//
// Implementation choice: Plan A (single mTLS endpoint).
// A single backend endpoint on 127.0.0.1:30052 is used.  The assertions focus
// on cookie lifecycle (set once, not re-set on follow-up requests) and on the
// HTTP version observed by the backend (HTTP/2.0 via /version endpoint).
// This avoids standing up a second mTLS backend port while still exercising
// the full gateway-to-backend TLS + H2 + sticky-cookie path.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;

pub struct H2MtlsPersistenceTestSuite;

impl H2MtlsPersistenceTestSuite {
    /// Verify that the Gateway successfully proxies a request to the mTLS backend
    /// and that the backend received the request over HTTP/2 (ALPN h2 negotiated).
    fn test_h2_mtls_handshake_and_routing() -> TestCase {
        TestCase::new(
            "h2_mtls_handshake_and_routing",
            "Gateway proxies to mTLS backend over HTTP/2 (ALPN h2); /version confirms HTTP/2.0",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Client -> Gateway is plain HTTP (no TLS on the listener side).
                    let client = reqwest::Client::builder()
                        .connect_timeout(std::time::Duration::from_secs(3))
                        .timeout(std::time::Duration::from_secs(8))
                        .no_proxy()
                        .redirect(reqwest::redirect::Policy::none())
                        .build()
                        .expect("Failed to create HTTP client");

                    // Use the /version endpoint: test_server responds with
                    // "Version: HTTP/2.0" when the gateway-to-backend leg used H2.
                    let url = format!("http://{}:{}/version", ctx.target_host, ctx.http_port);

                    let resp = match client
                        .get(&url)
                        .header("host", "h2-mtls-sp.example.com")
                        .header("x-trace-id", "h2-mtls-handshake")
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Request to /version failed: {}", e));
                        }
                    };

                    if resp.status() != 200 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Expected 200 from /version, got {}", resp.status()),
                        );
                    }

                    let body = match resp.text().await {
                        Ok(b) => b,
                        Err(e) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Failed to read /version response body: {}", e),
                            );
                        }
                    };

                    // The test_server version_handler formats: "Version: HTTP/2.0\n"
                    // when the gateway-to-backend connection used HTTP/2 (ALPN h2).
                    if !body.contains("HTTP/2.0") {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected backend to see HTTP/2.0 (ALPN h2 negotiated), \
                                 but /version response body was: {:?}",
                                body
                            ),
                        );
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        format!(
                            "Backend received request over HTTP/2 (ALPN h2 + mTLS): version_body={:?}",
                            body.trim()
                        ),
                    )
                })
            },
        )
    }

    /// Verify that the first request receives a Set-Cookie header containing
    /// the session cookie, and that subsequent requests carrying the cookie do
    /// NOT receive a new Set-Cookie (the Gateway recognises the existing session).
    fn test_session_persistence_cookie_set_and_reused() -> TestCase {
        TestCase::new(
            "session_persistence_cookie_set_and_reused",
            "First request gets Set-Cookie edgion-h2-mtls-sp; follow-up requests do NOT get a new Set-Cookie",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Use a client WITHOUT automatic cookie jar so we have full control
                    // over when the cookie is sent.
                    let client = reqwest::Client::builder()
                        .connect_timeout(std::time::Duration::from_secs(3))
                        .timeout(std::time::Duration::from_secs(8))
                        .no_proxy()
                        .redirect(reqwest::redirect::Policy::none())
                        .build()
                        .expect("Failed to create HTTP client");

                    let url = format!("http://{}:{}/echo", ctx.target_host, ctx.http_port);

                    // --- Step 1: first request (no cookie) ---
                    let resp1 = match client
                        .get(&url)
                        .header("host", "h2-mtls-sp.example.com")
                        .header("x-trace-id", "h2-mtls-sp-init")
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Initial /echo request failed: {}", e));
                        }
                    };

                    if resp1.status() != 200 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Expected 200 on initial request, got {}", resp1.status()),
                        );
                    }

                    // --- Step 2: verify Set-Cookie contains the session cookie ---
                    let set_cookie_hdr = resp1
                        .headers()
                        .get("set-cookie")
                        .and_then(|v| v.to_str().ok())
                        .map(|s| s.to_string());

                    let cookie_header = match &set_cookie_hdr {
                        Some(sc) if sc.starts_with("edgion-h2-mtls-sp=") => sc.clone(),
                        Some(sc) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Set-Cookie does not start with 'edgion-h2-mtls-sp=': {}", sc),
                            );
                        }
                        None => {
                            return TestResult::failed(
                                start.elapsed(),
                                "No Set-Cookie header on first request".to_string(),
                            );
                        }
                    };

                    // Extract the name=value portion to send in Cookie header.
                    let cookie_value = cookie_header.split(';').next().unwrap_or(&cookie_header).to_string();

                    // --- Step 3: subsequent requests WITH the cookie ---
                    // Expect 200 responses and NO new Set-Cookie (session already pinned).
                    for i in 0..3u32 {
                        let resp = match client
                            .get(&url)
                            .header("host", "h2-mtls-sp.example.com")
                            .header("cookie", &cookie_value)
                            .header("x-trace-id", format!("h2-mtls-sp-repeat-{}", i))
                            .send()
                            .await
                        {
                            Ok(r) => r,
                            Err(e) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Cookie request {} failed: {}", i, e),
                                );
                            }
                        };

                        if resp.status() != 200 {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Cookie request {} got unexpected status {}", i, resp.status()),
                            );
                        }

                        // A new Set-Cookie would mean the Gateway did not recognise the
                        // existing session — flag this as a test failure.
                        if let Some(sc) = resp.headers().get("set-cookie") {
                            let sc_str = sc.to_str().unwrap_or("<non-utf8>");
                            if sc_str.starts_with("edgion-h2-mtls-sp=") {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Request {} received an unexpected new Set-Cookie '{}'; \
                                         Gateway should reuse the existing session cookie",
                                        i, sc_str
                                    ),
                                );
                            }
                        }
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        format!(
                            "Session persistence OK: cookie={}, 3 follow-up requests succeeded \
                             with no new Set-Cookie",
                            cookie_value
                        ),
                    )
                })
            },
        )
    }

    /// Verify the /version endpoint confirms H2 on every request
    /// (including requests that carry the session cookie), proving that
    /// the ALPN h2 + mTLS path is stable across multiple requests in the
    /// same logical session.
    fn test_h2_protocol_stable_across_session() -> TestCase {
        TestCase::new(
            "h2_protocol_stable_across_session",
            "Backend sees HTTP/2.0 on all requests (with and without session cookie)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let client = reqwest::Client::builder()
                        .connect_timeout(std::time::Duration::from_secs(3))
                        .timeout(std::time::Duration::from_secs(8))
                        .no_proxy()
                        .redirect(reqwest::redirect::Policy::none())
                        .build()
                        .expect("Failed to create HTTP client");

                    let url = format!("http://{}:{}/version", ctx.target_host, ctx.http_port);

                    // First request: acquire session cookie.
                    let resp1 = match client
                        .get(&url)
                        .header("host", "h2-mtls-sp.example.com")
                        .header("x-trace-id", "h2-mtls-version-init")
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Initial /version request failed: {}", e),
                            );
                        }
                    };

                    if resp1.status() != 200 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Expected 200 from initial /version, got {}", resp1.status()),
                        );
                    }

                    // Extract cookie from Set-Cookie.
                    let cookie_value = resp1
                        .headers()
                        .get("set-cookie")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|sc| sc.split(';').next())
                        .map(|s| s.to_string());

                    let body1 = resp1.text().await.unwrap_or_default();
                    if !body1.contains("HTTP/2.0") {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Initial request: expected HTTP/2.0 in /version response, got: {:?}",
                                body1.trim()
                            ),
                        );
                    }

                    // Follow-up requests with the session cookie.
                    for i in 0..3u32 {
                        let mut req = client
                            .get(&url)
                            .header("host", "h2-mtls-sp.example.com")
                            .header("x-trace-id", format!("h2-mtls-version-{}", i));

                        if let Some(ref cv) = cookie_value {
                            req = req.header("cookie", cv.clone());
                        }

                        let resp = match req.send().await {
                            Ok(r) => r,
                            Err(e) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Follow-up /version request {} failed: {}", i, e),
                                );
                            }
                        };

                        if resp.status() != 200 {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Follow-up /version request {} got status {}", i, resp.status()),
                            );
                        }

                        let body = resp.text().await.unwrap_or_default();
                        if !body.contains("HTTP/2.0") {
                            return TestResult::failed(
                                start.elapsed(),
                                format!(
                                    "Follow-up request {}: expected HTTP/2.0 in /version response, \
                                     got: {:?}",
                                    i,
                                    body.trim()
                                ),
                            );
                        }
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        "HTTP/2.0 (ALPN h2) confirmed on all 4 /version requests \
                         (initial + 3 cookie-carrying follow-ups)"
                            .to_string(),
                    )
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for H2MtlsPersistenceTestSuite {
    fn name(&self) -> &str {
        "H2 Backend mTLS + SessionPersistence"
    }

    fn port_key(&self) -> Option<&str> {
        Some("HTTPRoute/Backend/H2BackendMtlsPersistence")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_h2_mtls_handshake_and_routing(),
            Self::test_session_persistence_cookie_set_and_reused(),
            Self::test_h2_protocol_stable_across_session(),
        ]
    }
}
