// EdgionPlugins CORS Plugin Test Suite
//
// Covers:
// - Preflight (OPTIONS) with matching / non-matching / wildcard origins
// - Simple requests (GET) with matching / non-matching / wildcard origins
// - Credentials mode (echo origin + Access-Control-Allow-Credentials)
// - Max-Age (preflight only)
// - Expose-Headers
// - Runtime-compiled origin matchers: comma-separated list,
//   HTTPS subdomain wildcard, and allow_origins_by_regex
//
// Configs defined in edgion-tests/integration/conf/EdgionPlugins/Cors/:
//   cors-basic:       specific origin, no credentials, maxAge=3600
//   cors-wildcard:    origin=*, no credentials
//   cors-credentials: specific origin, credentials=true, maxAge=7200
//   cors-regex:       comma-separated list + *.sub wildcard + regex, no credentials

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use reqwest::Method;
use std::time::Instant;

const HOST: &str = "cors-test.example.com";

pub struct CorsPluginTestSuite;

// ── helpers ──────────────────────────────────────────────────────────────

fn header_val(resp: &reqwest::Response, name: &str) -> Option<String> {
    resp.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

fn assert_header_eq(resp: &reqwest::Response, name: &str, expected: &str) -> Result<(), String> {
    match header_val(resp, name) {
        Some(v) if v == expected => Ok(()),
        Some(v) => Err(format!("Expected {}={}, got {}", name, expected, v)),
        None => Err(format!("Missing header {}, expected {}", name, expected)),
    }
}

fn assert_header_absent(resp: &reqwest::Response, name: &str) -> Result<(), String> {
    match header_val(resp, name) {
        Some(v) => Err(format!("Expected {} absent, got {}", name, v)),
        None => Ok(()),
    }
}

fn assert_header_present(resp: &reqwest::Response, name: &str) -> Result<(), String> {
    match header_val(resp, name) {
        Some(_) => Ok(()),
        None => Err(format!("Expected {} present", name)),
    }
}

fn assert_header_contains(resp: &reqwest::Response, name: &str, substring: &str) -> Result<(), String> {
    match header_val(resp, name) {
        Some(v) if v.contains(substring) => Ok(()),
        Some(v) => Err(format!("Expected {} to contain '{}', got '{}'", name, substring, v)),
        None => Err(format!("Missing header {}", name)),
    }
}

// ── test cases ───────────────────────────────────────────────────────────

impl CorsPluginTestSuite {
    // 1. Preflight with matching origin → 204 + full CORS headers
    fn test_preflight_matching_origin() -> TestCase {
        TestCase::new(
            "cors_preflight_matching_origin",
            "Preflight with matching origin returns 204 + CORS headers",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let resp = ctx
                        .http_client
                        .request(Method::OPTIONS, format!("{}/cors-basic/test", ctx.http_url()))
                        .header("host", HOST)
                        .header("origin", "https://app.example.com")
                        .header("access-control-request-method", "POST")
                        .send()
                        .await;

                    match resp {
                        Ok(resp) => {
                            if resp.status().as_u16() != 204 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 204, got {}", resp.status()),
                                );
                            }
                            if let Err(e) =
                                assert_header_eq(&resp, "access-control-allow-origin", "https://app.example.com")
                            {
                                return TestResult::failed(start.elapsed(), e);
                            }
                            if let Err(e) = assert_header_contains(&resp, "access-control-allow-methods", "GET") {
                                return TestResult::failed(start.elapsed(), e);
                            }
                            if let Err(e) =
                                assert_header_contains(&resp, "access-control-allow-headers", "Content-Type")
                            {
                                return TestResult::failed(start.elapsed(), e);
                            }
                            if let Err(e) = assert_header_eq(&resp, "access-control-max-age", "3600") {
                                return TestResult::failed(start.elapsed(), e);
                            }
                            if let Err(e) = assert_header_absent(&resp, "access-control-allow-credentials") {
                                return TestResult::failed(start.elapsed(), e);
                            }
                            TestResult::passed(start.elapsed())
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // 2. Simple GET with matching origin → 200 + CORS headers on proxied response
    fn test_simple_matching_origin() -> TestCase {
        TestCase::new(
            "cors_simple_matching_origin",
            "Simple GET with matching origin returns CORS headers",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let resp = ctx
                        .http_client
                        .get(format!("{}/cors-basic/headers", ctx.http_url()))
                        .header("host", HOST)
                        .header("origin", "https://app.example.com")
                        .send()
                        .await;

                    match resp {
                        Ok(resp) => {
                            if resp.status() != 200 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200, got {}", resp.status()),
                                );
                            }
                            if let Err(e) =
                                assert_header_eq(&resp, "access-control-allow-origin", "https://app.example.com")
                            {
                                return TestResult::failed(start.elapsed(), e);
                            }
                            // Max-Age should NOT be present on simple requests
                            if let Err(e) = assert_header_absent(&resp, "access-control-max-age") {
                                return TestResult::failed(start.elapsed(), e);
                            }
                            // Expose-Headers should be present
                            if let Err(e) =
                                assert_header_contains(&resp, "access-control-expose-headers", "X-Custom-Header")
                            {
                                return TestResult::failed(start.elapsed(), e);
                            }
                            TestResult::passed(start.elapsed())
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // 3. Preflight with non-matching origin → 204 without CORS headers
    fn test_preflight_non_matching_origin() -> TestCase {
        TestCase::new(
            "cors_preflight_non_matching_origin",
            "Preflight with non-matching origin returns 204 without CORS headers",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let resp = ctx
                        .http_client
                        .request(Method::OPTIONS, format!("{}/cors-basic/test", ctx.http_url()))
                        .header("host", HOST)
                        .header("origin", "https://evil.com")
                        .header("access-control-request-method", "POST")
                        .send()
                        .await;

                    match resp {
                        Ok(resp) => {
                            if resp.status().as_u16() != 204 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 204, got {}", resp.status()),
                                );
                            }
                            if let Err(e) = assert_header_absent(&resp, "access-control-allow-origin") {
                                return TestResult::failed(start.elapsed(), e);
                            }
                            if let Err(e) = assert_header_absent(&resp, "access-control-allow-methods") {
                                return TestResult::failed(start.elapsed(), e);
                            }
                            TestResult::passed(start.elapsed())
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // 4. Simple GET with non-matching origin → no CORS headers
    fn test_simple_non_matching_origin() -> TestCase {
        TestCase::new(
            "cors_simple_non_matching_origin",
            "Simple GET with non-matching origin returns no CORS headers",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let resp = ctx
                        .http_client
                        .get(format!("{}/cors-basic/headers", ctx.http_url()))
                        .header("host", HOST)
                        .header("origin", "https://evil.com")
                        .send()
                        .await;

                    match resp {
                        Ok(resp) => {
                            if resp.status() != 200 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200, got {}", resp.status()),
                                );
                            }
                            if let Err(e) = assert_header_absent(&resp, "access-control-allow-origin") {
                                return TestResult::failed(start.elapsed(), e);
                            }
                            TestResult::passed(start.elapsed())
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // 5. Wildcard origin preflight → ACAO: *
    fn test_wildcard_origin_preflight() -> TestCase {
        TestCase::new(
            "cors_wildcard_origin_preflight",
            "Wildcard origin preflight returns ACAO: *",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let resp = ctx
                        .http_client
                        .request(Method::OPTIONS, format!("{}/cors-wildcard/test", ctx.http_url()))
                        .header("host", HOST)
                        .header("origin", "https://any-site.example.org")
                        .header("access-control-request-method", "GET")
                        .send()
                        .await;

                    match resp {
                        Ok(resp) => {
                            if resp.status().as_u16() != 204 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 204, got {}", resp.status()),
                                );
                            }
                            if let Err(e) = assert_header_eq(&resp, "access-control-allow-origin", "*") {
                                return TestResult::failed(start.elapsed(), e);
                            }
                            // No Vary header when origin is * (EdgionPlugins behavior)
                            if let Err(e) = assert_header_absent(&resp, "vary") {
                                return TestResult::failed(start.elapsed(), e);
                            }
                            TestResult::passed(start.elapsed())
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // 6. Wildcard origin simple request → ACAO: *
    fn test_wildcard_origin_simple() -> TestCase {
        TestCase::new(
            "cors_wildcard_origin_simple",
            "Wildcard origin simple request returns ACAO: *",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let resp = ctx
                        .http_client
                        .get(format!("{}/cors-wildcard/headers", ctx.http_url()))
                        .header("host", HOST)
                        .header("origin", "https://any-site.example.org")
                        .send()
                        .await;

                    match resp {
                        Ok(resp) => {
                            if resp.status() != 200 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200, got {}", resp.status()),
                                );
                            }
                            if let Err(e) = assert_header_eq(&resp, "access-control-allow-origin", "*") {
                                return TestResult::failed(start.elapsed(), e);
                            }
                            TestResult::passed(start.elapsed())
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // 7. Credentials preflight → echo origin + ACAC: true
    fn test_credentials_preflight() -> TestCase {
        TestCase::new(
            "cors_credentials_preflight",
            "Credentials preflight echoes origin and sets ACAC: true",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let resp = ctx
                        .http_client
                        .request(Method::OPTIONS, format!("{}/cors-creds/test", ctx.http_url()))
                        .header("host", HOST)
                        .header("origin", "https://trusted.example.com")
                        .header("access-control-request-method", "PUT")
                        .header("access-control-request-headers", "Authorization")
                        .send()
                        .await;

                    match resp {
                        Ok(resp) => {
                            if resp.status().as_u16() != 204 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 204, got {}", resp.status()),
                                );
                            }
                            if let Err(e) =
                                assert_header_eq(&resp, "access-control-allow-origin", "https://trusted.example.com")
                            {
                                return TestResult::failed(start.elapsed(), e);
                            }
                            if let Err(e) = assert_header_eq(&resp, "access-control-allow-credentials", "true") {
                                return TestResult::failed(start.elapsed(), e);
                            }
                            if let Err(e) = assert_header_eq(&resp, "access-control-max-age", "7200") {
                                return TestResult::failed(start.elapsed(), e);
                            }
                            if let Err(e) = assert_header_present(&resp, "access-control-allow-methods") {
                                return TestResult::failed(start.elapsed(), e);
                            }
                            TestResult::passed(start.elapsed())
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // 8. Credentials simple request → echo origin + ACAC: true
    fn test_credentials_simple() -> TestCase {
        TestCase::new(
            "cors_credentials_simple",
            "Credentials simple request echoes origin and sets ACAC: true",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let resp = ctx
                        .http_client
                        .get(format!("{}/cors-creds/headers", ctx.http_url()))
                        .header("host", HOST)
                        .header("origin", "https://trusted.example.com")
                        .header("cookie", "session=abc123")
                        .send()
                        .await;

                    match resp {
                        Ok(resp) => {
                            if resp.status() != 200 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200, got {}", resp.status()),
                                );
                            }
                            if let Err(e) =
                                assert_header_eq(&resp, "access-control-allow-origin", "https://trusted.example.com")
                            {
                                return TestResult::failed(start.elapsed(), e);
                            }
                            if let Err(e) = assert_header_eq(&resp, "access-control-allow-credentials", "true") {
                                return TestResult::failed(start.elapsed(), e);
                            }
                            if let Err(e) =
                                assert_header_contains(&resp, "access-control-expose-headers", "X-Response-Id")
                            {
                                return TestResult::failed(start.elapsed(), e);
                            }
                            // Max-Age should NOT be on simple requests
                            if let Err(e) = assert_header_absent(&resp, "access-control-max-age") {
                                return TestResult::failed(start.elapsed(), e);
                            }
                            TestResult::passed(start.elapsed())
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // 9. Credentials preflight with non-matching origin → 204 without CORS headers
    fn test_credentials_non_matching_preflight() -> TestCase {
        TestCase::new(
            "cors_credentials_non_matching_preflight",
            "Credentials preflight with non-matching origin returns 204 without CORS headers",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let resp = ctx
                        .http_client
                        .request(Method::OPTIONS, format!("{}/cors-creds/test", ctx.http_url()))
                        .header("host", HOST)
                        .header("origin", "https://untrusted.example.com")
                        .header("access-control-request-method", "PUT")
                        .send()
                        .await;

                    match resp {
                        Ok(resp) => {
                            if resp.status().as_u16() != 204 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 204, got {}", resp.status()),
                                );
                            }
                            if let Err(e) = assert_header_absent(&resp, "access-control-allow-origin") {
                                return TestResult::failed(start.elapsed(), e);
                            }
                            if let Err(e) = assert_header_absent(&resp, "access-control-allow-credentials") {
                                return TestResult::failed(start.elapsed(), e);
                            }
                            TestResult::passed(start.elapsed())
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // 11. Regex preflight (cors-regex) → 204 + echoed origin.
    //     Exercises the cached Arc<CorsConfig> in the global preflight handler;
    //     the compiled regex must match.
    fn test_regex_preflight_matching() -> TestCase {
        TestCase::new(
            "cors_regex_preflight_matching",
            "Preflight from a regex-matched origin returns 204 + echoed origin",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let resp = ctx
                        .http_client
                        .request(Method::OPTIONS, format!("{}/cors-regex/test", ctx.http_url()))
                        .header("host", HOST)
                        .header("origin", "https://svc.regex.example.com")
                        .header("access-control-request-method", "POST")
                        .send()
                        .await;

                    match resp {
                        Ok(resp) => {
                            if resp.status().as_u16() != 204 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 204, got {}", resp.status()),
                                );
                            }
                            if let Err(e) =
                                assert_header_eq(&resp, "access-control-allow-origin", "https://svc.regex.example.com")
                            {
                                return TestResult::failed(start.elapsed(), e);
                            }
                            TestResult::passed(start.elapsed())
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // 12. Regex simple GET (cors-regex) → 200 + echoed origin.
    //     Exercises the per-route request-filter Cors::new path.
    fn test_regex_simple_matching() -> TestCase {
        TestCase::new(
            "cors_regex_simple_matching",
            "Simple GET from a regex-matched origin returns echoed origin",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let resp = ctx
                        .http_client
                        .get(format!("{}/cors-regex/headers", ctx.http_url()))
                        .header("host", HOST)
                        .header("origin", "https://api.regex.example.com")
                        .send()
                        .await;

                    match resp {
                        Ok(resp) => {
                            if resp.status() != 200 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200, got {}", resp.status()),
                                );
                            }
                            if let Err(e) =
                                assert_header_eq(&resp, "access-control-allow-origin", "https://api.regex.example.com")
                            {
                                return TestResult::failed(start.elapsed(), e);
                            }
                            TestResult::passed(start.elapsed())
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // 13. Subdomain-wildcard simple GET (cors-regex `*.sub.example.com`) → echoed origin.
    fn test_subdomain_wildcard_simple() -> TestCase {
        TestCase::new(
            "cors_subdomain_wildcard_simple",
            "Simple GET from an HTTPS subdomain-wildcard origin returns echoed origin",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let resp = ctx
                        .http_client
                        .get(format!("{}/cors-regex/headers", ctx.http_url()))
                        .header("host", HOST)
                        .header("origin", "https://tenant.sub.example.com")
                        .send()
                        .await;

                    match resp {
                        Ok(resp) => {
                            if resp.status() != 200 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200, got {}", resp.status()),
                                );
                            }
                            if let Err(e) =
                                assert_header_eq(&resp, "access-control-allow-origin", "https://tenant.sub.example.com")
                            {
                                return TestResult::failed(start.elapsed(), e);
                            }
                            TestResult::passed(start.elapsed())
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // 14. Comma-separated exact origin (cors-regex `https://a.example.com,...`) → echoed origin.
    //     Exercises the origins_cache HashMap path.
    fn test_comma_list_exact_simple() -> TestCase {
        TestCase::new(
            "cors_comma_list_exact_simple",
            "Simple GET from an exact origin in a comma-separated list returns echoed origin",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let resp = ctx
                        .http_client
                        .get(format!("{}/cors-regex/headers", ctx.http_url()))
                        .header("host", HOST)
                        .header("origin", "https://a.example.com")
                        .send()
                        .await;

                    match resp {
                        Ok(resp) => {
                            if resp.status() != 200 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200, got {}", resp.status()),
                                );
                            }
                            if let Err(e) =
                                assert_header_eq(&resp, "access-control-allow-origin", "https://a.example.com")
                            {
                                return TestResult::failed(start.elapsed(), e);
                            }
                            TestResult::passed(start.elapsed())
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // 15. Non-matching origin against cors-regex → no CORS headers (guards against
    //     the compiled regex over-matching / echoing every origin).
    fn test_regex_non_matching() -> TestCase {
        TestCase::new(
            "cors_regex_non_matching",
            "Simple GET from an origin matching neither list nor regex returns no CORS headers",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let resp = ctx
                        .http_client
                        .get(format!("{}/cors-regex/headers", ctx.http_url()))
                        .header("host", HOST)
                        .header("origin", "https://evil.com")
                        .send()
                        .await;

                    match resp {
                        Ok(resp) => {
                            if resp.status() != 200 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200, got {}", resp.status()),
                                );
                            }
                            if let Err(e) = assert_header_absent(&resp, "access-control-allow-origin") {
                                return TestResult::failed(start.elapsed(), e);
                            }
                            TestResult::passed(start.elapsed())
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // 10. No Origin header → no CORS processing, normal proxy
    fn test_no_origin_header() -> TestCase {
        TestCase::new(
            "cors_no_origin_header",
            "Request without Origin header is not CORS-processed",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let resp = ctx
                        .http_client
                        .get(format!("{}/cors-basic/headers", ctx.http_url()))
                        .header("host", HOST)
                        .send()
                        .await;

                    match resp {
                        Ok(resp) => {
                            if resp.status() != 200 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200, got {}", resp.status()),
                                );
                            }
                            if let Err(e) = assert_header_absent(&resp, "access-control-allow-origin") {
                                return TestResult::failed(start.elapsed(), e);
                            }
                            TestResult::passed(start.elapsed())
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }
}

impl TestSuite for CorsPluginTestSuite {
    fn name(&self) -> &str {
        "CORS Plugin Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_preflight_matching_origin(),
            Self::test_simple_matching_origin(),
            Self::test_preflight_non_matching_origin(),
            Self::test_simple_non_matching_origin(),
            Self::test_wildcard_origin_preflight(),
            Self::test_wildcard_origin_simple(),
            Self::test_credentials_preflight(),
            Self::test_credentials_simple(),
            Self::test_credentials_non_matching_preflight(),
            Self::test_no_origin_header(),
            Self::test_regex_preflight_matching(),
            Self::test_regex_simple_matching(),
            Self::test_subdomain_wildcard_simple(),
            Self::test_comma_list_exact_simple(),
            Self::test_regex_non_matching(),
        ]
    }
}
