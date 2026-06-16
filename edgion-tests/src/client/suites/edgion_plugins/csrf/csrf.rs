// CSRF Plugin Integration Test Suite
//
// Tests the CSRF protection plugin (double-submit cookie pattern).
// The plugin generates a signed token cookie on safe methods (GET/HEAD/OPTIONS),
// and on unsafe methods (POST/PUT/DELETE) validates that:
//   1. A CSRF token header is present (name: "X-CSRF-Token")
//   2. A CSRF cookie with the same name is present
//   3. Header value == cookie value
//   4. Token signature is valid and not expired
//
// Required config files (in edgion-tests/integration/conf/EdgionPlugins/Csrf/):
//   - 01_Secret_default_csrf-key.yaml                    # Secret holding the CSRF signing key
//   - EdgionPlugins_default_csrf-test.yaml               # Csrf plugin with secretRef/name config
//   - HTTPRoute_default_csrf-test.yaml                   # Route: csrf-test.example.com
//   - EdgionPlugins_default_csrf-missing-secret.yaml     # Csrf plugin whose secretRef Secret is never created
//   - HTTPRoute_default_csrf-missing-secret.yaml         # Route: csrf-missing-secret.example.com (fail-closed)

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};

pub struct CsrfTestSuite;

const HOST: &str = "csrf-test.example.com";
const TOKEN_HEADER: &str = "X-CSRF-Token";
/// Host routed to a Csrf plugin whose `secretRef` names a Secret that no
/// fixture creates. The gateway must seal the plugin fail-closed (reject with
/// 500) rather than serve CSRF with an empty key.
const MISSING_SECRET_HOST: &str = "csrf-missing-secret.example.com";

/// Extract the CSRF token value from a Set-Cookie header string.
/// The cookie looks like: X-CSRF-Token=<value>; Path=/; ...
fn extract_csrf_token(set_cookie: &str, name: &str) -> Option<String> {
    for part in set_cookie.split(';') {
        let part = part.trim();
        if let Some(value) = part.strip_prefix(&format!("{}=", name)) {
            return Some(value.to_string());
        }
    }
    None
}

impl CsrfTestSuite {
    /// GET request → 200 + Set-Cookie with CSRF token in response
    fn test_safe_method_sets_cookie() -> TestCase {
        TestCase::new(
            "safe_method_sets_cookie",
            "GET request skips CSRF validation and sets token cookie in response",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = std::time::Instant::now();
                    let url = format!("{}/health", ctx.http_url());

                    let resp = match ctx.http_client.get(&url).header("host", HOST).send().await {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };

                    let status = resp.status().as_u16();
                    if status != 200 {
                        return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                    }

                    let cookie_header = resp
                        .headers()
                        .get("set-cookie")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("");

                    if cookie_header.contains(TOKEN_HEADER) {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            format!("GET returned 200 with Set-Cookie containing {}", TOKEN_HEADER),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!("Expected Set-Cookie with {}, got: {:?}", TOKEN_HEADER, cookie_header),
                        )
                    }
                })
            },
        )
    }

    /// GET an upstream path that returns two Set-Cookie headers; the CSRF plugin
    /// (safe method) appends its own. All three Set-Cookie values must survive.
    /// Regression for the insert_header-clobbers-upstream-Set-Cookie bug, where
    /// the queued CSRF cookie replaced every upstream Set-Cookie.
    fn test_upstream_cookies_preserved() -> TestCase {
        TestCase::new(
            "upstream_set_cookies_preserved",
            "CSRF cookie is appended without wiping the upstream's Set-Cookie values",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = std::time::Instant::now();
                    // Upstream /multi-set-cookie returns upstream_a + upstream_b.
                    let url = format!("{}/multi-set-cookie", ctx.http_url());

                    let resp = match ctx.http_client.get(&url).header("host", HOST).send().await {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };

                    let status = resp.status().as_u16();
                    if status != 200 {
                        return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                    }

                    // Collect ALL Set-Cookie values (get_all, not get → first only).
                    let cookies: Vec<String> = resp
                        .headers()
                        .get_all("set-cookie")
                        .iter()
                        .filter_map(|v| v.to_str().ok())
                        .map(String::from)
                        .collect();

                    let has_a = cookies.iter().any(|c| c.contains("upstream_a=aaa"));
                    let has_b = cookies.iter().any(|c| c.contains("upstream_b=bbb"));
                    let has_csrf = cookies.iter().any(|c| c.contains(TOKEN_HEADER));

                    if has_a && has_b && has_csrf {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            format!("All three Set-Cookie values present ({} total)", cookies.len()),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected upstream_a + upstream_b + {} in Set-Cookie, got: {:?}",
                                TOKEN_HEADER, cookies
                            ),
                        )
                    }
                })
            },
        )
    }

    /// POST with valid CSRF token (header + cookie match, valid signature) → 200
    fn test_valid_csrf_token_passes() -> TestCase {
        TestCase::new(
            "valid_csrf_token_passes",
            "POST with matching header+cookie CSRF token returns 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = std::time::Instant::now();
                    let get_url = format!("{}/health", ctx.http_url());
                    // /echo accepts both GET and POST; /health is GET-only
                    let post_url = format!("{}/echo", ctx.http_url());

                    // Step 1: GET to obtain a fresh CSRF token cookie
                    let get_resp = match ctx.http_client.get(&get_url).header("host", HOST).send().await {
                        Ok(r) => r,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("GET (token fetch) failed: {}", e))
                        }
                    };

                    if get_resp.status().as_u16() != 200 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("GET returned {} instead of 200", get_resp.status()),
                        );
                    }

                    let set_cookie = match get_resp
                        .headers()
                        .get("set-cookie")
                        .and_then(|v| v.to_str().ok())
                        .map(String::from)
                    {
                        Some(v) => v,
                        None => {
                            return TestResult::failed(start.elapsed(), "GET response missing Set-Cookie header".into())
                        }
                    };

                    let token = match extract_csrf_token(&set_cookie, TOKEN_HEADER) {
                        Some(t) => t,
                        None => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!(
                                    "Could not extract {} token from Set-Cookie: {}",
                                    TOKEN_HEADER, set_cookie
                                ),
                            )
                        }
                    };

                    // Step 2: POST with token in both header and cookie
                    let cookie_val = format!("{}={}", TOKEN_HEADER, token);
                    let post_resp = match ctx
                        .http_client
                        .post(&post_url)
                        .header("host", HOST)
                        .header("cookie", &cookie_val)
                        .header(TOKEN_HEADER, &token)
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("POST failed: {}", e)),
                    };

                    let status = post_resp.status().as_u16();
                    if status == 200 {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "POST with valid CSRF token returned 200".into(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!("Expected 200 for valid CSRF token, got {}", status),
                        )
                    }
                })
            },
        )
    }

    /// POST without CSRF header → 401
    fn test_missing_header_returns_401() -> TestCase {
        TestCase::new(
            "missing_csrf_header_returns_401",
            "POST without CSRF header returns 401",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = std::time::Instant::now();
                    let url = format!("{}/health", ctx.http_url());

                    let resp = match ctx.http_client.post(&url).header("host", HOST).send().await {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };

                    let status = resp.status().as_u16();
                    if status == 401 {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "POST without CSRF header correctly returned 401".into(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!("Expected 401 for missing CSRF header, got {}", status),
                        )
                    }
                })
            },
        )
    }

    /// POST with header but no cookie → 401
    fn test_missing_cookie_returns_401() -> TestCase {
        TestCase::new(
            "missing_csrf_cookie_returns_401",
            "POST with CSRF header but no cookie returns 401",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = std::time::Instant::now();
                    let url = format!("{}/health", ctx.http_url());

                    let resp = match ctx
                        .http_client
                        .post(&url)
                        .header("host", HOST)
                        .header(TOKEN_HEADER, "some-token-value")
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };

                    let status = resp.status().as_u16();
                    if status == 401 {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "POST with header but no cookie correctly returned 401".into(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!("Expected 401 for missing CSRF cookie, got {}", status),
                        )
                    }
                })
            },
        )
    }

    /// POST with mismatched header and cookie tokens → 401
    fn test_mismatched_tokens_returns_401() -> TestCase {
        TestCase::new(
            "mismatched_tokens_returns_401",
            "POST with non-matching header and cookie CSRF tokens returns 401",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = std::time::Instant::now();
                    let url = format!("{}/health", ctx.http_url());

                    let resp = match ctx
                        .http_client
                        .post(&url)
                        .header("host", HOST)
                        .header("cookie", format!("{}=cookie-token-aaa", TOKEN_HEADER))
                        .header(TOKEN_HEADER, "header-token-bbb")
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };

                    let status = resp.status().as_u16();
                    if status == 401 {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "POST with mismatched tokens correctly returned 401".into(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!("Expected 401 for mismatched tokens, got {}", status),
                        )
                    }
                })
            },
        )
    }

    /// POST with matching but invalid (unsigned/corrupted) tokens → 401
    fn test_invalid_signature_returns_401() -> TestCase {
        TestCase::new(
            "invalid_token_signature_returns_401",
            "POST with matching but unsigned CSRF tokens returns 401",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = std::time::Instant::now();
                    let url = format!("{}/health", ctx.http_url());

                    // A value that matches header==cookie but is not a valid signed token
                    let fake_token = "not-a-valid-csrf-token";
                    let resp = match ctx
                        .http_client
                        .post(&url)
                        .header("host", HOST)
                        .header("cookie", format!("{}={}", TOKEN_HEADER, fake_token))
                        .header(TOKEN_HEADER, fake_token)
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };

                    let status = resp.status().as_u16();
                    if status == 401 {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "POST with invalid token signature correctly returned 401".into(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!("Expected 401 for invalid token signature, got {}", status),
                        )
                    }
                })
            },
        )
    }

    /// A Csrf plugin whose secretRef points at a Secret that is never created
    /// must be sealed fail-closed: any request to the route is rejected with
    /// 500, NOT served with an empty signing key. This exercises the gateway
    /// gate in plugin_runtime.rs::plugin_validation_error (CSRF with no
    /// resolved_key → seal_fail_closed → RejectFilter → 500).
    fn test_missing_secret_fails_closed() -> TestCase {
        TestCase::new(
            "missing_secret_fails_closed",
            "Csrf plugin with an unresolvable secretRef is sealed fail-closed (500), not served with an empty key",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = std::time::Instant::now();
                    // GET is a safe method that normally sets a CSRF cookie; if the
                    // plugin were served with an empty key it would return 200 here.
                    let url = format!("{}/health", ctx.http_url());

                    let resp = match ctx
                        .http_client
                        .get(&url)
                        .header("host", MISSING_SECRET_HOST)
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };

                    let status = resp.status().as_u16();
                    if status == 500 {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "Csrf plugin with missing Secret correctly returned 500 (fail-closed)".into(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected 500 (fail-closed) for unresolvable CSRF secretRef, got {}",
                                status
                            ),
                        )
                    }
                })
            },
        )
    }
}

impl TestSuite for CsrfTestSuite {
    fn name(&self) -> &str {
        "Csrf"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_safe_method_sets_cookie(),
            Self::test_upstream_cookies_preserved(),
            Self::test_valid_csrf_token_passes(),
            Self::test_missing_header_returns_401(),
            Self::test_missing_cookie_returns_401(),
            Self::test_mismatched_tokens_returns_401(),
            Self::test_invalid_signature_returns_401(),
            Self::test_missing_secret_fails_closed(),
        ]
    }
}
