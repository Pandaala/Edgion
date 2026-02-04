// Header Modifier Test suite
//
// Test RequestHeaderModifier and ResponseHeaderModifier filters
//
// Test scenarios:
// 1. RequestHeaderModifier: set/add/remove request headers
// 2. ResponseHeaderModifier: set/add/remove response headers
// 3. Combined: both request and response header modifications
//
// Required config: HTTPRoute/Filters/HeaderModifier/HTTPRoute.yaml

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;

const TEST_HOST: &str = "header-test.example.com";

pub struct HeaderModifierTestSuite;

impl HeaderModifierTestSuite {
    /// Test RequestHeaderModifier - set header
    fn test_request_header_set() -> TestCase {
        TestCase::new(
            "request_header_set",
            "Test RequestHeaderModifier set operation",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Request /headers endpoint which echoes back received headers
                    let url = format!("{}/request-header-test", ctx.http_url());

                    match ctx.http_client.get(&url).header("Host", TEST_HOST).send().await {
                        Ok(response) => {
                            if !response.status().is_success() {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 2xx, got {}", response.status()),
                                );
                            }

                            // The /headers endpoint returns JSON with received headers
                            match response.text().await {
                                Ok(body) => {
                                    // Check if X-Set-Header is present in the echoed headers
                                    if body.contains("X-Set-Header") && body.contains("set-value") {
                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            "RequestHeaderModifier set operation verified".to_string(),
                                        )
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!("X-Set-Header not found in response: {}", body),
                                        )
                                    }
                                }
                                Err(e) => TestResult::failed(start.elapsed(), format!("Failed to read body: {}", e)),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test RequestHeaderModifier - add header
    fn test_request_header_add() -> TestCase {
        TestCase::new(
            "request_header_add",
            "Test RequestHeaderModifier add operation",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let url = format!("{}/request-header-test", ctx.http_url());

                    match ctx.http_client.get(&url).header("Host", TEST_HOST).send().await {
                        Ok(response) => {
                            if !response.status().is_success() {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 2xx, got {}", response.status()),
                                );
                            }

                            match response.text().await {
                                Ok(body) => {
                                    if body.contains("X-Add-Header") && body.contains("add-value") {
                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            "RequestHeaderModifier add operation verified".to_string(),
                                        )
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!("X-Add-Header not found in response: {}", body),
                                        )
                                    }
                                }
                                Err(e) => TestResult::failed(start.elapsed(), format!("Failed to read body: {}", e)),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test RequestHeaderModifier - remove header
    fn test_request_header_remove() -> TestCase {
        TestCase::new(
            "request_header_remove",
            "Test RequestHeaderModifier remove operation",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let url = format!("{}/request-header-test", ctx.http_url());

                    // Send request WITH the header that should be removed
                    match ctx
                        .http_client
                        .get(&url)
                        .header("Host", TEST_HOST)
                        .header("X-Remove-Me", "should-be-removed")
                        .send()
                        .await
                    {
                        Ok(response) => {
                            if !response.status().is_success() {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 2xx, got {}", response.status()),
                                );
                            }

                            match response.text().await {
                                Ok(body) => {
                                    // The header should NOT be present in the echoed headers
                                    if !body.contains("X-Remove-Me") && !body.contains("should-be-removed") {
                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            "RequestHeaderModifier remove operation verified".to_string(),
                                        )
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!("X-Remove-Me should have been removed: {}", body),
                                        )
                                    }
                                }
                                Err(e) => TestResult::failed(start.elapsed(), format!("Failed to read body: {}", e)),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test ResponseHeaderModifier - set header
    fn test_response_header_set() -> TestCase {
        TestCase::new(
            "response_header_set",
            "Test ResponseHeaderModifier set operation",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let url = format!("{}/response-header-test", ctx.http_url());

                    match ctx.http_client.get(&url).header("Host", TEST_HOST).send().await {
                        Ok(response) => {
                            if !response.status().is_success() {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 2xx, got {}", response.status()),
                                );
                            }

                            // Check response headers directly
                            match response.headers().get("X-Response-Set") {
                                Some(value) => {
                                    if value.to_str().unwrap_or("") == "resp-set-value" {
                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            "ResponseHeaderModifier set operation verified".to_string(),
                                        )
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!("X-Response-Set has wrong value: {}", value.to_str().unwrap_or("")),
                                        )
                                    }
                                }
                                None => TestResult::failed(
                                    start.elapsed(),
                                    "X-Response-Set header not found in response".to_string(),
                                ),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test ResponseHeaderModifier - add header
    fn test_response_header_add() -> TestCase {
        TestCase::new(
            "response_header_add",
            "Test ResponseHeaderModifier add operation",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let url = format!("{}/response-header-test", ctx.http_url());

                    match ctx.http_client.get(&url).header("Host", TEST_HOST).send().await {
                        Ok(response) => {
                            if !response.status().is_success() {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 2xx, got {}", response.status()),
                                );
                            }

                            match response.headers().get("X-Response-Add") {
                                Some(value) => {
                                    if value.to_str().unwrap_or("") == "resp-add-value" {
                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            "ResponseHeaderModifier add operation verified".to_string(),
                                        )
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!("X-Response-Add has wrong value: {}", value.to_str().unwrap_or("")),
                                        )
                                    }
                                }
                                None => TestResult::failed(
                                    start.elapsed(),
                                    "X-Response-Add header not found in response".to_string(),
                                ),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test combined RequestHeaderModifier and ResponseHeaderModifier
    fn test_combined_modifiers() -> TestCase {
        TestCase::new(
            "combined_modifiers",
            "Test both RequestHeaderModifier and ResponseHeaderModifier together",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let url = format!("{}/both-headers-test", ctx.http_url());

                    match ctx.http_client.get(&url).header("Host", TEST_HOST).send().await {
                        Ok(response) => {
                            if !response.status().is_success() {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 2xx, got {}", response.status()),
                                );
                            }

                            // Check response header (ResponseHeaderModifier)
                            let response_header_ok = response
                                .headers()
                                .get("X-Processed-By")
                                .map(|v| v.to_str().unwrap_or("") == "edgion-gateway")
                                .unwrap_or(false);

                            // Get body to check request header was modified
                            match response.text().await {
                                Ok(body) => {
                                    let request_header_ok =
                                        body.contains("X-Request-Id") && body.contains("test-request-123");

                                    if response_header_ok && request_header_ok {
                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            "Both request and response header modifiers working".to_string(),
                                        )
                                    } else if !response_header_ok {
                                        TestResult::failed(
                                            start.elapsed(),
                                            "X-Processed-By response header not found or wrong value".to_string(),
                                        )
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!("X-Request-Id not found in echoed headers: {}", body),
                                        )
                                    }
                                }
                                Err(e) => TestResult::failed(start.elapsed(), format!("Failed to read body: {}", e)),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test multiple request headers modification
    fn test_multi_request_headers() -> TestCase {
        TestCase::new(
            "multi_request_headers",
            "Test multiple request headers set and add",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let url = format!("{}/multi-request-headers", ctx.http_url());

                    match ctx.http_client.get(&url).header("Host", TEST_HOST).send().await {
                        Ok(response) => {
                            if !response.status().is_success() {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 2xx, got {}", response.status()),
                                );
                            }

                            match response.text().await {
                                Ok(body) => {
                                    let mut found_headers = Vec::new();
                                    let mut missing_headers = Vec::new();

                                    // Check all expected headers
                                    let expected = [
                                        ("X-Custom-Auth", "Bearer token123"),
                                        ("X-Request-Source", "gateway"),
                                        ("X-Trace-Id", "trace-abc"),
                                        ("X-Span-Id", "span-xyz"),
                                    ];

                                    for (name, value) in expected {
                                        if body.contains(name) && body.contains(value) {
                                            found_headers.push(name);
                                        } else {
                                            missing_headers.push(name);
                                        }
                                    }

                                    if missing_headers.is_empty() {
                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            format!("All {} request headers verified", found_headers.len()),
                                        )
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            format!("Missing headers: {:?}", missing_headers),
                                        )
                                    }
                                }
                                Err(e) => TestResult::failed(start.elapsed(), format!("Failed to read body: {}", e)),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test multiple response headers modification
    fn test_multi_response_headers() -> TestCase {
        TestCase::new(
            "multi_response_headers",
            "Test multiple response headers set and add",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    let url = format!("{}/multi-response-headers", ctx.http_url());

                    match ctx.http_client.get(&url).header("Host", TEST_HOST).send().await {
                        Ok(response) => {
                            if !response.status().is_success() {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 2xx, got {}", response.status()),
                                );
                            }

                            let headers = response.headers();
                            let mut found_headers = Vec::new();
                            let mut missing_headers = Vec::new();

                            // Check expected response headers
                            let expected = [
                                ("cache-control", "no-cache, no-store"),
                                ("x-content-type-options", "nosniff"),
                                ("x-custom-header-1", "value1"),
                                ("x-custom-header-2", "value2"),
                            ];

                            for (name, value) in expected {
                                if let Some(header_value) = headers.get(name) {
                                    if header_value.to_str().unwrap_or("") == value {
                                        found_headers.push(name);
                                    } else {
                                        missing_headers.push(format!(
                                            "{} (wrong value: {})",
                                            name,
                                            header_value.to_str().unwrap_or("")
                                        ));
                                    }
                                } else {
                                    missing_headers.push(name.to_string());
                                }
                            }

                            if missing_headers.is_empty() {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("All {} response headers verified", found_headers.len()),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Missing/wrong headers: {:?}", missing_headers),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for HeaderModifierTestSuite {
    fn name(&self) -> &str {
        "Header Modifier"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_request_header_set(),
            Self::test_request_header_add(),
            Self::test_request_header_remove(),
            Self::test_response_header_set(),
            Self::test_response_header_add(),
            Self::test_combined_modifiers(),
            Self::test_multi_request_headers(),
            Self::test_multi_response_headers(),
        ]
    }
}
