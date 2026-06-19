// ProxyRewrite Plugin Test Suite
//
// Covers:
// - URI rewrite: simple, $uri variable, $arg_xxx query variable, regex
// - Combined rewrite: Host + Headers (add/set/remove)
// - Path parameter extraction via $uid
//
// test_server echo response format:
// Server: 0.0.0.0:30001
// Client: 127.0.0.1:xxxxx
// Method: GET
// Path: /xxx
// Headers:
//   Host: xxx
//   X-Custom: xxx

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::collections::HashMap;
use std::time::Instant;

pub struct ProxyRewriteTestSuite;

/// Parse the plain-text echo response returned by test_server.
fn parse_echo_response(text: &str) -> EchoInfo {
    let mut info = EchoInfo::default();
    let mut in_headers = false;

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if line == "Headers:" {
            in_headers = true;
            continue;
        }

        if in_headers {
            // Inside the Headers block, each line has two-space indent: "  Name: Value"
            if let Some(pos) = line.find(':') {
                let name = line[..pos].trim().to_lowercase();
                let value = line[pos + 1..].trim().to_string();
                info.headers.insert(name, value);
            }
        } else {
            // Before the Headers block, each line is "Field: Value"
            if let Some(pos) = line.find(':') {
                let field = line[..pos].trim();
                let value = line[pos + 1..].trim();
                match field {
                    "Method" => info.method = value.to_string(),
                    "Path" => info.path = value.to_string(),
                    _ => {}
                }
            }
        }
    }

    info
}

#[derive(Debug, Default)]
struct EchoInfo {
    method: String,
    path: String,
    headers: HashMap<String, String>,
}

impl ProxyRewriteTestSuite {
    // ==================== 1. URI simple rewrite ====================
    fn test_uri_simple() -> TestCase {
        TestCase::new(
            "uri_simple",
            "URI : /uri/simple/test -> /internal/api/v2",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/uri/simple/test", ctx.http_url());

                    let request = client.get(&url).header("host", "proxy-rewrite.example.com");

                    match request.send().await {
                        Ok(response) => {
                            if !response.status().is_success() {
                                return TestResult::failed(start.elapsed(), format!("HTTP {}", response.status()));
                            }

                            match response.text().await {
                                Ok(text) => {
                                    let echo = parse_echo_response(&text);
                                    if echo.path == "/internal/api/v2" {
                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            format!("URI -> {}", echo.path),
                                        )
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!("Expected /internal/api/v2, got {}", echo.path),
                                        )
                                    }
                                }
                                Err(e) => TestResult::failed(start.elapsed(), format!("Read error: {}", e)),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 2. URI with $uri variable ====================
    fn test_uri_var() -> TestCase {
        TestCase::new(
            "uri_var",
            "URI $uri : /uri/var/test -> /prefix/uri/var/test/suffix",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/uri/var/test", ctx.http_url());

                    let request = client.get(&url).header("host", "proxy-rewrite.example.com");

                    match request.send().await {
                        Ok(response) => {
                            if !response.status().is_success() {
                                return TestResult::failed(start.elapsed(), format!("HTTP {}", response.status()));
                            }

                            match response.text().await {
                                Ok(text) => {
                                    let echo = parse_echo_response(&text);
                                    let expected = "/prefix/uri/var/test/suffix";
                                    if echo.path == expected {
                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            format!("URI -> {}", echo.path),
                                        )
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!("Expected {}, got {}", expected, echo.path),
                                        )
                                    }
                                }
                                Err(e) => TestResult::failed(start.elapsed(), format!("Read error: {}", e)),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 3. URI with $arg_xxx query variable ====================
    fn test_uri_arg() -> TestCase {
        TestCase::new(
            "uri_arg",
            "URI $arg : ?keyword=hello&lang=en -> /search/hello/en",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/uri/arg/test?keyword=hello&lang=en", ctx.http_url());

                    let request = client.get(&url).header("host", "proxy-rewrite.example.com");

                    match request.send().await {
                        Ok(response) => {
                            if !response.status().is_success() {
                                return TestResult::failed(start.elapsed(), format!("HTTP {}", response.status()));
                            }

                            match response.text().await {
                                Ok(text) => {
                                    let echo = parse_echo_response(&text);
                                    // Path starts with /search/hello/en (query string consumed into the path)
                                    if echo.path.starts_with("/search/hello/en") {
                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            format!("URI -> {}", echo.path),
                                        )
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!("Expected /search/hello/en*, got {}", echo.path),
                                        )
                                    }
                                }
                                Err(e) => TestResult::failed(start.elapsed(), format!("Read error: {}", e)),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 3b. URI + $arg_xxx (percent-encoded input) ====================

    /// Verify that $arg_keyword with a percent-encoded value produces a
    /// single-encoded upstream URL, not double-encoded. Input keyword is
    /// `hello%20world` (decoded: "hello world").
    ///
    /// `req.uri().path()` on the upstream side returns the path as-received
    /// (percent-encoded form preserved), so:
    ///   - Correct (single-encode): plugin decodes `?keyword=hello%20world`
    ///     to "hello world", re-encodes once when substituting `$arg_keyword`,
    ///     upstream Path is `/search/hello%20world/en`.
    ///   - Regression (double-encode): plugin re-encodes the already-encoded
    ///     value, emitting `%2520` (the `%` itself encoded), so upstream Path
    ///     would be `/search/hello%2520world/en`.
    fn test_uri_arg_percent_encoded() -> TestCase {
        TestCase::new(
            "uri_arg_percent_encoded",
            "URI $arg with percent-encoded input: no double-encoding (Fix 1)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    // %20 is the percent-encoding of a space; the decoded keyword is "hello world".
                    let url = format!("{}/uri/arg/test?keyword=hello%20world&lang=en", ctx.http_url());

                    let request = client.get(&url).header("host", "proxy-rewrite.example.com");

                    match request.send().await {
                        Ok(response) => {
                            if !response.status().is_success() {
                                return TestResult::failed(start.elapsed(), format!("HTTP {}", response.status()));
                            }

                            match response.text().await {
                                Ok(text) => {
                                    let echo = parse_echo_response(&text);
                                    // Correct single-encode: upstream sees the value encoded exactly once.
                                    if echo.path == "/search/hello%20world/en" {
                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            format!("No double-encoding: Path={}", echo.path),
                                        )
                                    } else if echo.path.contains("%25") {
                                        // Any `%25` proves the `%` itself was encoded — i.e.
                                        // the substitution step encoded an already-encoded value.
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!("REGRESSION double-encoding detected: Path={}", echo.path),
                                        )
                                    } else {
                                        TestResult::failed(start.elapsed(), format!("Unexpected path: {}", echo.path))
                                    }
                                }
                                Err(e) => TestResult::failed(start.elapsed(), format!("Read error: {}", e)),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 4. Regex URI rewrite ====================
    fn test_regex_uri() -> TestCase {
        TestCase::new(
            "regex_uri",
            "Regex URI: /regex/users/12345 -> /user-service/12345",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/regex/users/12345", ctx.http_url());

                    let request = client.get(&url).header("host", "proxy-rewrite.example.com");

                    match request.send().await {
                        Ok(response) => {
                            if !response.status().is_success() {
                                return TestResult::failed(start.elapsed(), format!("HTTP {}", response.status()));
                            }

                            match response.text().await {
                                Ok(text) => {
                                    let echo = parse_echo_response(&text);
                                    if echo.path == "/user-service/12345" {
                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            format!("URI -> {}", echo.path),
                                        )
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!("Expected /user-service/12345, got {}", echo.path),
                                        )
                                    }
                                }
                                Err(e) => TestResult::failed(start.elapsed(), format!("Read error: {}", e)),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 5. Combined rewrite (Host + Headers) ====================
    fn test_combined() -> TestCase {
        TestCase::new("combined", ": Host + Headers(add/set/remove)", |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();
                let client = &ctx.http_client;
                let url = format!("{}/combined/test", ctx.http_url());

                let request = client
                    .get(&url)
                    .header("host", "proxy-rewrite.example.com")
                    .header("x-debug", "should-be-removed")
                    .header("x-internal-token", "secret");

                match request.send().await {
                    Ok(response) => {
                        if !response.status().is_success() {
                            return TestResult::failed(start.elapsed(), format!("HTTP {}", response.status()));
                        }

                        match response.text().await {
                            Ok(text) => {
                                let echo = parse_echo_response(&text);
                                let mut errors = Vec::new();

                                // Host rewrite
                                if let Some(host) = echo.headers.get("host") {
                                    if host != "backend.internal.svc" {
                                        errors.push(format!("Host: expected backend.internal.svc, got {}", host));
                                    }
                                } else {
                                    errors.push("Host header not found".to_string());
                                }

                                // Headers add
                                if echo.headers.get("x-gateway") != Some(&"edgion".to_string()) {
                                    errors.push(format!(
                                        "X-Gateway: expected edgion, got {:?}",
                                        echo.headers.get("x-gateway")
                                    ));
                                }

                                // Headers set
                                if let Some(path) = echo.headers.get("x-original-path") {
                                    if !path.contains("/combined") {
                                        errors.push(format!("X-Original-Path: wrong value {}", path));
                                    }
                                } else {
                                    errors.push("X-Original-Path not set".to_string());
                                }

                                // Headers remove
                                if echo.headers.contains_key("x-debug") {
                                    errors.push("X-Debug not removed".to_string());
                                }
                                if echo.headers.contains_key("x-internal-token") {
                                    errors.push("X-Internal-Token not removed".to_string());
                                }

                                if errors.is_empty() {
                                    TestResult::passed_with_message(
                                        start.elapsed(),
                                        format!(
                                            "Host={:?}, X-Gateway={:?}",
                                            echo.headers.get("host"),
                                            echo.headers.get("x-gateway")
                                        ),
                                    )
                                } else {
                                    TestResult::failed(start.elapsed(), errors.join("; "))
                                }
                            }
                            Err(e) => TestResult::failed(start.elapsed(), format!("Read error: {}", e)),
                        }
                    }
                    Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                }
            })
        })
    }

    // ==================== 6b. Path-param × Header — always url-encoded ====================

    /// ProxyRewrite path-param ($<name>) header rewrite always url-encodes the
    /// decoded path-param value in Header context, mirroring $arg_<name>'s rule.
    /// /params/foo+bar/info -> URI=/user-service/foo%2Bbar/profile, X-User-Id=foo%2Bbar.
    fn test_path_param_header_url_encoded() -> TestCase {
        TestCase::new(
            "path_param_header_url_encoded",
            "ProxyRewrite path-param header: $<name> always url-encoded (this PR)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/params/foo+bar/info", ctx.http_url());
                    let request = client.get(&url).header("host", "proxy-rewrite.example.com");

                    match request.send().await {
                        Ok(response) => {
                            if !response.status().is_success() {
                                return TestResult::failed(start.elapsed(), format!("HTTP {}", response.status()));
                            }
                            match response.text().await {
                                Ok(text) => {
                                    let echo = parse_echo_response(&text);
                                    let mut errors = Vec::new();

                                    // URI context: $uid is always url-encoded in URI templates.
                                    if echo.path != "/user-service/foo%2Bbar/profile" {
                                        errors.push(format!(
                                            "URI: expected /user-service/foo%2Bbar/profile, got {}",
                                            echo.path
                                        ));
                                    }
                                    // Header context: $<name> must url-encode the decoded
                                    // path-param value, same as $arg_<name>.
                                    if echo.headers.get("x-user-id") != Some(&"foo%2Bbar".to_string()) {
                                        errors.push(format!(
                                            "X-User-Id: expected foo%2Bbar, got {:?}",
                                            echo.headers.get("x-user-id")
                                        ));
                                    }

                                    if errors.is_empty() {
                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            format!("URI={}, X-User-Id={:?}", echo.path, echo.headers.get("x-user-id")),
                                        )
                                    } else {
                                        TestResult::failed(start.elapsed(), errors.join("; "))
                                    }
                                }
                                Err(e) => TestResult::failed(start.elapsed(), format!("Read error: {}", e)),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 7. Header + $arg_<name> always url-encoded ====================

    /// ProxyRewrite header: $arg_<name> always url-encodes the decoded query value.
    /// ?id=a+b: '+' decodes to space, re-encoded as %20 → X-Arg-Id: a%20b.
    /// ?id=user-123: ASCII identity → X-Arg-Id: user-123.
    fn test_header_arg_always_encoded() -> TestCase {
        TestCase::new(
            "header_arg_always_encoded",
            "ProxyRewrite header: $arg_<name> always url-encoded (Fix 3)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;

                    // Case 1: ?id=a+b  →  decoded "a b"  →  url-encoded "a%20b"
                    let resp = client
                        .get(format!("{}/request-header-test?id=a+b", ctx.http_url()))
                        .header("host", "proxy-rewrite.example.com")
                        .send()
                        .await;
                    let body = match resp {
                        Ok(r) if r.status().is_success() => r.text().await.unwrap_or_default(),
                        Ok(r) => {
                            return TestResult::failed(start.elapsed(), format!("Expected 2xx, got {}", r.status()))
                        }
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };
                    if !body.contains("X-Arg-Id: a%20b") {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Expected `X-Arg-Id: a%20b` in echoed headers; body=\n{}", body),
                        );
                    }

                    // Case 2: ?id=user-123  →  ASCII identity  →  X-Arg-Id: user-123
                    let resp2 = client
                        .get(format!("{}/request-header-test?id=user-123", ctx.http_url()))
                        .header("host", "proxy-rewrite.example.com")
                        .send()
                        .await;
                    let body2 = match resp2 {
                        Ok(r) if r.status().is_success() => r.text().await.unwrap_or_default(),
                        Ok(r) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Expected 2xx (identity), got {}", r.status()),
                            )
                        }
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Identity request failed: {}", e))
                        }
                    };
                    if body2.contains("X-Arg-Id: user-123") {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "$arg_id url-encoded for `+`->space and identity for ASCII".to_string(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!("Expected `X-Arg-Id: user-123` (identity); body=\n{}", body2),
                        )
                    }
                })
            },
        )
    }

    // ==================== 8. Header + $1 (capture) always url-encoded ====================

    /// ProxyRewrite header: $1 (regex capture) always url-encodes, mirroring $arg_<name>.
    /// /capture-header/foo+bar:    captured bytes "foo+bar" → X-Trace: foo%2Bbar
    /// /capture-header/abc-xyz:    ASCII identity            → X-Trace: abc-xyz
    fn test_header_capture_always_encoded() -> TestCase {
        TestCase::new(
            "header_capture_always_encoded",
            "ProxyRewrite header: $1 (regex capture) always url-encoded",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;

                    // Case 1: '+' in the path segment is captured as a literal '+'
                    // (paths don't decode '+' as space) and must be re-encoded as %2B.
                    let resp = client
                        .get(format!("{}/capture-header/foo+bar", ctx.http_url()))
                        .header("host", "proxy-rewrite.example.com")
                        .send()
                        .await;
                    let body = match resp {
                        Ok(r) if r.status().is_success() => r.text().await.unwrap_or_default(),
                        Ok(r) => {
                            return TestResult::failed(start.elapsed(), format!("Expected 2xx, got {}", r.status()))
                        }
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };
                    if !body.contains("X-Trace: foo%2Bbar") {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Expected `X-Trace: foo%2Bbar` in echoed headers; body=\n{}", body),
                        );
                    }

                    // Case 2: ASCII-safe capture — identity output.
                    let resp2 = client
                        .get(format!("{}/capture-header/abc-xyz", ctx.http_url()))
                        .header("host", "proxy-rewrite.example.com")
                        .send()
                        .await;
                    let body2 = match resp2 {
                        Ok(r) if r.status().is_success() => r.text().await.unwrap_or_default(),
                        Ok(r) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Expected 2xx (identity), got {}", r.status()),
                            )
                        }
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Identity request failed: {}", e))
                        }
                    };
                    if body2.contains("X-Trace: abc-xyz") {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "$1 url-encoded for `+`->%2B and identity for ASCII".to_string(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!("Expected `X-Trace: abc-xyz` (identity); body=\n{}", body2),
                        )
                    }
                })
            },
        )
    }

    // ==================== 6. Path parameter extraction ====================
    fn test_path_param() -> TestCase {
        TestCase::new(
            "path_param",
            "Path : /params/789/info -> URI=/user-service/789/profile, Header X-User-Id=789",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/params/789/info", ctx.http_url());

                    let request = client.get(&url).header("host", "proxy-rewrite.example.com");

                    match request.send().await {
                        Ok(response) => {
                            if !response.status().is_success() {
                                return TestResult::failed(start.elapsed(), format!("HTTP {}", response.status()));
                            }

                            match response.text().await {
                                Ok(text) => {
                                    let echo = parse_echo_response(&text);
                                    let mut errors = Vec::new();

                                    // URI rewrite with $uid
                                    if echo.path != "/user-service/789/profile" {
                                        errors.push(format!(
                                            "URI: expected /user-service/789/profile, got {}",
                                            echo.path
                                        ));
                                    }

                                    // Header rewrite with $uid
                                    if echo.headers.get("x-user-id") != Some(&"789".to_string()) {
                                        errors.push(format!(
                                            "X-User-Id: expected 789, got {:?}",
                                            echo.headers.get("x-user-id")
                                        ));
                                    }

                                    if errors.is_empty() {
                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            format!("URI={}, X-User-Id={:?}", echo.path, echo.headers.get("x-user-id")),
                                        )
                                    } else {
                                        TestResult::failed(start.elapsed(), errors.join("; "))
                                    }
                                }
                                Err(e) => TestResult::failed(start.elapsed(), format!("Read error: {}", e)),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }
}

impl TestSuite for ProxyRewriteTestSuite {
    fn name(&self) -> &str {
        "ProxyRewrite Plugin Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_uri_simple(),
            Self::test_uri_var(),
            Self::test_uri_arg(),
            // Percent-encoded $arg: verify no double-encoding
            Self::test_uri_arg_percent_encoded(),
            Self::test_regex_uri(),
            // Path parameter extraction via `:uid` route syntax.
            // The matched HTTPRoute uses `value: /params/:uid/info` so the radix
            // matcher records `uid` in `ctx.path_params`, which ProxyRewrite then
            // substitutes into URI / header templates as `$uid`.
            Self::test_path_param(),
            // Same fixture but with a non-ASCII input (foo+bar) to verify that
            // $<name> in Header context is url-encoded. URI context is also pinned.
            Self::test_path_param_header_url_encoded(),
            // Header $arg_<name> always url-encoded
            Self::test_header_arg_always_encoded(),
            // Header $1 (regex capture) always url-encoded
            Self::test_header_capture_always_encoded(),
        ]
    }
}
