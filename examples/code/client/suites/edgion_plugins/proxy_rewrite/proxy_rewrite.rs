// ProxyRewrite Plugin Test Suite
//
// ：
// - URI ：（）
// - ：Host + Method + Headers 
// - Path ： $uid 
//
// test_server ：
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

///  test_server 
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
            //  Header: "  Name: Value"
            if let Some(pos) = line.find(':') {
                let name = line[..pos].trim().to_lowercase();
                let value = line[pos + 1..].trim().to_string();
                info.headers.insert(name, value);
            }
        } else {
            // : "Field: Value"
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
    // ==================== 1. URI  ====================
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

    // ==================== 2. URI + $uri  ====================
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

    // ==================== 3. URI + $arg_xxx  ====================
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
                                    // : /search/hello/en ( query string  path )
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

    // ==================== 4. Regex URI  ====================
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

    // ==================== 5.  ====================
    fn test_combined() -> TestCase {
        TestCase::new(
            "combined",
            ": Host + Headers(add/set/remove)",
            |ctx: TestContext| {
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

                                    //  Host 
                                    if let Some(host) = echo.headers.get("host") {
                                        if host != "backend.internal.svc" {
                                            errors.push(format!("Host: expected backend.internal.svc, got {}", host));
                                        }
                                    } else {
                                        errors.push("Host header not found".to_string());
                                    }

                                    //  Headers add
                                    if echo.headers.get("x-gateway") != Some(&"edgion".to_string()) {
                                        errors.push(format!(
                                            "X-Gateway: expected edgion, got {:?}",
                                            echo.headers.get("x-gateway")
                                        ));
                                    }

                                    //  Headers set
                                    if let Some(path) = echo.headers.get("x-original-path") {
                                        if !path.contains("/combined") {
                                            errors.push(format!("X-Original-Path: wrong value {}", path));
                                        }
                                    } else {
                                        errors.push("X-Original-Path not set".to_string());
                                    }

                                    //  Headers remove
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
            },
        )
    }

    // ==================== 6. Path  ====================
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

                                    //  URI  $uid 
                                    if echo.path != "/user-service/789/profile" {
                                        errors.push(format!(
                                            "URI: expected /user-service/789/profile, got {}",
                                            echo.path
                                        ));
                                    }

                                    //  Header  $uid 
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
            Self::test_regex_uri(),
            // combined  path_param ：
            // - combined: test_server  Headers， Headers 
            // - path_param: （:uid ）
        ]
    }
}
