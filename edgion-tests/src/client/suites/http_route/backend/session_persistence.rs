// ============================================================================
// Session Persistence Test Suite
// ============================================================================
//
// Verifies Gateway API Session Persistence (Cookie and Header modes).
//
// Test scenarios:
// 1. Cookie persistence - first request gets Set-Cookie, subsequent requests
//    with that cookie are routed successfully
// 2. Header persistence - first request gets session header in response,
//    subsequent requests with that header are routed successfully

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::collections::HashSet;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub struct SessionPersistenceTestSuite;

/// Current Unix time in whole seconds (clock read, NOT a sleep).
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Parse a session token of the form `<16hex>.<expiry_epoch>`. Returns
/// `(addrhash, expiry)`. Returns `None` if it is not the expiry-bearing form.
fn parse_token_expiry(token: &str) -> Option<(&str, u64)> {
    let (hash, exp) = token.split_once('.')?;
    let expiry: u64 = exp.parse().ok()?;
    if hash.len() == 16 && hash.chars().all(|c| c.is_ascii_hexdigit()) {
        Some((hash, expiry))
    } else {
        None
    }
}

/// Consume the response so status, affinity headers and backend identity are all checked.
async fn cookie_wire_response(response: reqwest::Response) -> Result<(String, Option<String>), String> {
    if response.status() != 200 {
        return Err(format!("expected HTTP 200, got {}", response.status()));
    }
    let mut cookies = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok()?.split(';').next()?.strip_prefix("edgion-sp-test="));
    let token = cookies.next().map(str::to_owned);
    if cookies.next().is_some() {
        return Err("multiple affinity Set-Cookie fields in one response".into());
    }
    let body = response
        .text()
        .await
        .map_err(|e| format!("reading echo response: {e}"))?;
    let addr = body
        .lines()
        .find_map(|line| line.strip_prefix("Server: "))
        .filter(|addr| !addr.is_empty())
        .ok_or("echo response has no backend identity")?;
    Ok((addr.to_owned(), token))
}

async fn cookie_wire_scenarios(ctx: TestContext) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .http1_only()
        .connect_timeout(std::time::Duration::from_secs(2))
        .timeout(std::time::Duration::from_secs(5))
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| format!("building HTTP client: {e}"))?;
    let request = || {
        client
            .get(format!("{}/test", ctx.http_url()))
            .header("host", "sp-cookie.example.com")
    };
    let mut seen = HashSet::new();
    let mut issued = None;
    // Prove the topology can distinguish backends before making any sticky assertion.
    for _ in 0..20 {
        let response = request().send().await.map_err(|e| format!("control request: {e}"))?;
        let (addr, token) = cookie_wire_response(response).await?;
        let token = token.ok_or("control request did not issue an affinity cookie")?;
        if parse_token_expiry(&token).is_none_or(|(_, expiry)| expiry <= now_secs()) {
            return Err("issued affinity token has no valid future expiry".into());
        }
        seen.insert(addr.clone());
        issued = Some((addr, token));
    }
    if seen.len() < 2 {
        return Err(format!("control requests reached fewer than two backends: {seen:?}"));
    }
    let (issuing_addr, token) = issued.ok_or("no control response")?;
    for _ in 0..10 {
        // HeaderMap::append keeps these as distinct H1 fields, in this order.
        let mut headers = reqwest::header::HeaderMap::new();
        headers.append("cookie", reqwest::header::HeaderValue::from_static("unrelated=first"));
        headers.append(
            "cookie",
            format!("edgion-sp-test={token}")
                .parse()
                .map_err(|e| format!("building affinity Cookie: {e}"))?,
        );
        let response = request()
            .headers(headers)
            .send()
            .await
            .map_err(|e| format!("second-field request: {e}"))?;
        let (addr, refreshed) = cookie_wire_response(response).await?;
        if addr != issuing_addr || refreshed.is_some() {
            return Err(format!(
                "second Cookie field: backend={addr}, expected={issuing_addr}, refreshed={}",
                refreshed.is_some()
            ));
        }
    }
    let (hash, _) = parse_token_expiry(&token).ok_or("invalid control token")?;
    for suffix in ["invalid", "", "18446744073709551616", "1.2"] {
        // A known live hash is essential: an arbitrary hash would miss even with the old bug.
        let response = request()
            .header("cookie", format!("edgion-sp-test={hash}.{suffix}"))
            .send()
            .await
            .map_err(|e| format!("malformed-expiry request: {e}"))?;
        let (addr, refreshed) = cookie_wire_response(response).await?;
        let refreshed = refreshed.ok_or_else(|| format!("malformed expiry {suffix:?} did not refresh"))?;
        if parse_token_expiry(&refreshed).is_none_or(|(_, expiry)| expiry <= now_secs()) {
            return Err(format!(
                "malformed expiry {suffix:?} did not produce a valid future expiry"
            ));
        }
        // Refresh need not change the hash or timestamp when it selects the same backend.
        // Prove the newly issued cookie actually pins its issuing backend without another refresh.
        for _ in 0..5 {
            let response = request()
                .header("cookie", format!("edgion-sp-test={refreshed}"))
                .send()
                .await
                .map_err(|e| format!("refreshed-cookie request: {e}"))?;
            let (replay_addr, reissued) = cookie_wire_response(response).await?;
            if replay_addr != addr || reissued.is_some() {
                return Err(format!(
                    "refreshed cookie for suffix {suffix:?} did not remain pinned without reissue"
                ));
            }
        }
    }
    Ok("Second Cookie field pins its issuing backend without refresh; four malformed expiry forms refresh and remain pinned".into())
}

impl SessionPersistenceTestSuite {
    fn test_cookie_fields_and_malformed_expiry() -> TestCase {
        TestCase::new(
            "cookie_fields_and_malformed_expiry",
            "Repeated Cookie fields and malformed expiry on HTTP/1.1",
            |ctx| {
                Box::pin(async move {
                    let start = Instant::now();
                    match cookie_wire_scenarios(ctx).await {
                        Ok(message) => TestResult::passed_with_message(start.elapsed(), message),
                        Err(error) => TestResult::failed(start.elapsed(), error),
                    }
                })
            },
        )
    }

    /// Test Cookie-based session persistence
    fn test_cookie_persistence() -> TestCase {
        TestCase::new(
            "cookie_persistence",
            "Verify Cookie session persistence - Set-Cookie on first request, routing with cookie",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let client = reqwest::Client::builder()
                        .connect_timeout(std::time::Duration::from_secs(2))
                        .timeout(std::time::Duration::from_secs(5))
                        .no_proxy()
                        .redirect(reqwest::redirect::Policy::none())
                        .build()
                        .expect("Failed to create HTTP client");

                    // Step 1: Send initial request WITHOUT cookie
                    let url = format!("http://{}:31313/test", ctx.target_host);
                    let resp = match client
                        .get(&url)
                        .header("host", "sp-cookie.example.com")
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker("sp-cookie-init"),
                        )
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Initial request failed: {}", e));
                        }
                    };

                    if resp.status() != 200 {
                        return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", resp.status()));
                    }

                    // Step 2: Verify Set-Cookie header
                    let set_cookie = resp
                        .headers()
                        .get("set-cookie")
                        .and_then(|v| v.to_str().ok())
                        .map(|s| s.to_string());

                    let cookie_header = match &set_cookie {
                        Some(sc) if sc.starts_with("edgion-sp-test=") => sc.clone(),
                        Some(sc) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Set-Cookie does not start with 'edgion-sp-test=': {}", sc),
                            );
                        }
                        None => {
                            return TestResult::failed(start.elapsed(), "No Set-Cookie header in response".to_string());
                        }
                    };

                    // Step 3: Verify cookie attributes
                    let mut missing_attrs = Vec::new();
                    if !cookie_header.contains("Path=/") {
                        missing_attrs.push("Path=/");
                    }
                    if !cookie_header.contains("Max-Age=") {
                        missing_attrs.push("Max-Age=");
                    }
                    if !cookie_header.contains("HttpOnly") {
                        missing_attrs.push("HttpOnly");
                    }
                    if !missing_attrs.is_empty() {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Set-Cookie missing attributes {:?}: {}", missing_attrs, cookie_header),
                        );
                    }

                    // Step 4: Extract cookie name=value for subsequent requests
                    let cookie_value = cookie_header.split(';').next().unwrap_or(&cookie_header).to_string();

                    // Step 5: Send 5 requests WITH the cookie, verify all succeed
                    for i in 0..5 {
                        let resp = client
                            .get(&url)
                            .header("host", "sp-cookie.example.com")
                            .header("cookie", &cookie_value)
                            .header(
                                "traceparent",
                                crate::access_log_client::traceparent_for_marker(format!("sp-cookie-{}", i)),
                            )
                            .send()
                            .await;

                        match resp {
                            Ok(r) if r.status() == 200 => {}
                            Ok(r) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Cookie request {} got status {}", i, r.status()),
                                );
                            }
                            Err(e) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Cookie request {} failed: {}", i, e),
                                );
                            }
                        }
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        format!(
                            "Cookie persistence OK: Set-Cookie={}, 5 subsequent requests succeeded",
                            cookie_value
                        ),
                    )
                })
            },
        )
    }

    /// Test Header-based session persistence
    fn test_header_persistence() -> TestCase {
        TestCase::new(
            "header_persistence",
            "Verify Header session persistence - response header on first request, routing with header",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let client = reqwest::Client::builder()
                        .connect_timeout(std::time::Duration::from_secs(2))
                        .timeout(std::time::Duration::from_secs(5))
                        .no_proxy()
                        .redirect(reqwest::redirect::Policy::none())
                        .build()
                        .expect("Failed to create HTTP client");

                    // Step 1: Send initial request WITHOUT session header
                    let url = format!("http://{}:31313/test", ctx.target_host);
                    let resp = match client
                        .get(&url)
                        .header("host", "sp-header.example.com")
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker("sp-header-init"),
                        )
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Initial request failed: {}", e));
                        }
                    };

                    if resp.status() != 200 {
                        return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", resp.status()));
                    }

                    // Step 2: Verify x-session-id header in response
                    let token = match resp
                        .headers()
                        .get("x-session-id")
                        .and_then(|v| v.to_str().ok())
                        .map(|s| s.to_string())
                    {
                        Some(t) if !t.is_empty() => t,
                        Some(_) => {
                            return TestResult::failed(start.elapsed(), "x-session-id header is empty".to_string());
                        }
                        None => {
                            return TestResult::failed(
                                start.elapsed(),
                                "No x-session-id header in response".to_string(),
                            );
                        }
                    };

                    // Step 3: Send 5 requests WITH the session header, verify all succeed
                    for i in 0..5 {
                        let resp = client
                            .get(&url)
                            .header("host", "sp-header.example.com")
                            .header("x-session-id", &token)
                            .header(
                                "traceparent",
                                crate::access_log_client::traceparent_for_marker(format!("sp-header-{}", i)),
                            )
                            .send()
                            .await;

                        match resp {
                            Ok(r) if r.status() == 200 => {}
                            Ok(r) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Header request {} got status {}", i, r.status()),
                                );
                            }
                            Err(e) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Header request {} failed: {}", i, e),
                                );
                            }
                        }
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        format!(
                            "Header persistence OK: token={}, 5 subsequent requests succeeded",
                            token
                        ),
                    )
                })
            },
        )
    }

    /// Cookie mode: the emitted token carries an absolute expiry, an expired
    /// (forged past-expiry) cookie is refreshed, and a live cookie is not
    /// re-issued. No wall-clock sleep — the expired state is forged, not waited
    /// for.
    fn test_cookie_absolute_expiry_and_refresh() -> TestCase {
        TestCase::new(
            "cookie_absolute_expiry_and_refresh",
            "Verify Cookie absolute-lifetime: token carries expiry, expired cookie refreshes, live cookie is not re-issued",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = reqwest::Client::builder()
                        .connect_timeout(std::time::Duration::from_secs(2))
                        .timeout(std::time::Duration::from_secs(5))
                        .no_proxy()
                        .redirect(reqwest::redirect::Policy::none())
                        .build()
                        .expect("Failed to create HTTP client");
                    let url = format!("http://{}:31313/test", ctx.target_host);

                    let get_set_cookie = |resp: &reqwest::Response| -> Option<String> {
                        resp.headers()
                            .get("set-cookie")
                            .and_then(|v| v.to_str().ok())
                            .map(|s| s.to_string())
                    };

                    // Step 1: initial request, no cookie -> Set-Cookie with an
                    // expiry-bearing token and Max-Age (Permanent lifetime).
                    let resp = match client
                        .get(&url)
                        .header("host", "sp-cookie.example.com")
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("initial request failed: {e}")),
                    };
                    let set_cookie = match get_set_cookie(&resp) {
                        Some(sc) => sc,
                        None => return TestResult::failed(start.elapsed(), "no Set-Cookie on initial request".into()),
                    };
                    let cookie_value = set_cookie.split(';').next().unwrap_or(&set_cookie).to_string();
                    let token = cookie_value.split_once('=').map(|(_, v)| v.to_string()).unwrap_or_default();
                    let (hash, expiry) = match parse_token_expiry(&token) {
                        Some(v) => v,
                        None => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("token is not <16hex>.<expiry>: {token}"),
                            );
                        }
                    };
                    if expiry <= now_secs() {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("issued token expiry {expiry} is not in the future (now {})", now_secs()),
                        );
                    }

                    // Step 2: replay a FORGED past-expiry cookie -> gateway treats
                    // it as expired and issues a fresh token (refresh).
                    let forged = format!("edgion-sp-test={hash}.1");
                    let resp = match client
                        .get(&url)
                        .header("host", "sp-cookie.example.com")
                        .header("cookie", &forged)
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("forged-cookie request failed: {e}")),
                    };
                    match get_set_cookie(&resp).as_deref().map(|sc| {
                        let v = sc.split(';').next().unwrap_or(sc);
                        v.split_once('=').and_then(|(_, t)| parse_token_expiry(t).map(|(_, e)| e))
                    }) {
                        Some(Some(new_exp)) if new_exp > now_secs() => {}
                        other => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("expired cookie was not refreshed with a future expiry: {other:?}"),
                            );
                        }
                    }

                    // Step 3: replay the LIVE cookie -> valid, so NO new Set-Cookie.
                    let resp = match client
                        .get(&url)
                        .header("host", "sp-cookie.example.com")
                        .header("cookie", &cookie_value)
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("live-cookie request failed: {e}")),
                    };
                    if let Some(sc) = get_set_cookie(&resp) {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("live cookie should not be re-issued, but got Set-Cookie: {sc}"),
                        );
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        "Cookie absolute-lifetime OK: token carries expiry, expired refreshes, live not re-issued".into(),
                    )
                })
            },
        )
    }

    /// Header mode: same three assertions, token travels in the `x-session-id`
    /// response/request header (no Max-Age). Requires `absoluteTimeout` on the
    /// header route.
    fn test_header_absolute_expiry_and_refresh() -> TestCase {
        TestCase::new(
            "header_absolute_expiry_and_refresh",
            "Verify Header absolute-lifetime: token carries expiry, expired header refreshes, live header is not re-issued",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = reqwest::Client::builder()
                        .connect_timeout(std::time::Duration::from_secs(2))
                        .timeout(std::time::Duration::from_secs(5))
                        .no_proxy()
                        .redirect(reqwest::redirect::Policy::none())
                        .build()
                        .expect("Failed to create HTTP client");
                    let url = format!("http://{}:31313/test", ctx.target_host);

                    let get_token = |resp: &reqwest::Response| -> Option<String> {
                        resp.headers()
                            .get("x-session-id")
                            .and_then(|v| v.to_str().ok())
                            .map(|s| s.to_string())
                    };

                    // Step 1: initial request -> x-session-id with expiry.
                    let resp = match client
                        .get(&url)
                        .header("host", "sp-header.example.com")
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("initial request failed: {e}")),
                    };
                    let token = match get_token(&resp) {
                        Some(t) => t,
                        None => return TestResult::failed(start.elapsed(), "no x-session-id on initial request".into()),
                    };
                    let (hash, expiry) = match parse_token_expiry(&token) {
                        Some(v) => v,
                        None => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("header token is not <16hex>.<expiry>: {token}"),
                            );
                        }
                    };
                    if expiry <= now_secs() {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("issued header token expiry {expiry} is not in the future", ),
                        );
                    }

                    // Step 2: forged past-expiry header -> refreshed.
                    let forged = format!("{hash}.1");
                    let resp = match client
                        .get(&url)
                        .header("host", "sp-header.example.com")
                        .header("x-session-id", &forged)
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("forged-header request failed: {e}")),
                    };
                    match get_token(&resp).and_then(|t| parse_token_expiry(&t).map(|(_, e)| e)) {
                        Some(new_exp) if new_exp > now_secs() => {}
                        other => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("expired header was not refreshed with a future expiry: {other:?}"),
                            );
                        }
                    }

                    // Step 3: live header -> NOT re-issued.
                    let resp = match client
                        .get(&url)
                        .header("host", "sp-header.example.com")
                        .header("x-session-id", &token)
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("live-header request failed: {e}")),
                    };
                    if let Some(t) = get_token(&resp) {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("live header should not be re-issued, but got x-session-id: {t}"),
                        );
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        "Header absolute-lifetime OK: token carries expiry, expired refreshes, live not re-issued".into(),
                    )
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for SessionPersistenceTestSuite {
    fn name(&self) -> &str {
        "Session Persistence Tests"
    }

    fn port_key(&self) -> Option<&str> {
        Some("HTTPRoute/Backend/SessionPersistence")
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_cookie_persistence(),
            Self::test_header_persistence(),
            Self::test_cookie_absolute_expiry_and_refresh(),
            Self::test_header_absolute_expiry_and_refresh(),
            Self::test_cookie_fields_and_malformed_expiry(),
        ]
    }
}
