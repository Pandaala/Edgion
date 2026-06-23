// HTTP Match Rules Test suite
//
// Required config files (in edgion-tests/integration/conf/):
// - HTTPRoute_default_match-test.yaml    # match rules test route (contains 8 rules)
// - HTTPRoute_default_section-test.yaml  # SectionName test route
// - EndpointSlice_edge_test-http.yaml    # HTTP backend service discovery
// - Service_edge_test-http.yaml          # HTTP service definition
// - Gateway_edge_example-gateway.yaml    # Gateway config
// - GatewayClass__public-gateway.yaml    # GatewayClass config

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use std::time::Instant;

pub struct HttpMatchTestSuite;

impl HttpMatchTestSuite {
    /// Test PathPrefix path match
    fn test_path_prefix_match() -> TestCase {
        TestCase::new("path_prefix_match", "Test PathPrefix path match", |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();

                // Positive test: should match prefix
                let mut request = ctx.http_client.get(format!("{}/api/v1/users", ctx.http_url()));
                request = request.header("Host", "match-test.example.com");

                match request.send().await {
                    Ok(response) => {
                        if response.status().is_success() {
                            TestResult::passed_with_message(
                                start.elapsed(),
                                format!("PathPrefix match successful: {}", response.status()),
                            )
                        } else {
                            TestResult::failed(start.elapsed(), format!("Expected 200, got {}", response.status()))
                        }
                    }
                    Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                }
            })
        })
    }

    /// Test Exact path match
    fn test_exact_path_match() -> TestCase {
        TestCase::new("exact_path_match", "Test Exact path match", |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();

                // Positive test: exact match
                let mut request = ctx.http_client.get(format!("{}/exact/path", ctx.http_url()));
                request = request.header("Host", "match-test.example.com");

                match request.send().await {
                    Ok(response) => {
                        if !response.status().is_success() {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Exact path match failed: {}", response.status()),
                            );
                        }
                    }
                    Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                }

                // Negative test: extra path content should not match
                let mut request = ctx.http_client.get(format!("{}/exact/path/extra", ctx.http_url()));
                request = request.header("Host", "match-test.example.com");

                match request.send().await {
                    Ok(response) => {
                        if response.status() == 404 {
                            TestResult::passed_with_message(
                                start.elapsed(),
                                "Exact path match works correctly (positive and negative tests passed)".to_string(),
                            )
                        } else {
                            TestResult::failed(
                                start.elapsed(),
                                format!("Expected 404 for non-exact path, got {}", response.status()),
                            )
                        }
                    }
                    Err(e) => TestResult::failed(start.elapsed(), format!("Negative test request failed: {}", e)),
                }
            })
        })
    }

    /// Test RegularExpression regex path match
    fn test_regex_path_match() -> TestCase {
        TestCase::new(
            "regex_path_match",
            "Test RegularExpression regex path match",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Positive test: match /users/123 (digits)
                    let mut request = ctx.http_client.get(format!("{}/users/123", ctx.http_url()));
                    request = request.header("Host", "match-test.example.com");

                    match request.send().await {
                        Ok(response) => {
                            if !response.status().is_success() {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Regex path match failed for /users/123: {}", response.status()),
                                );
                            }
                        }
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }

                    // Negative: letters do not match the digits-only regex
                    let mut request = ctx.http_client.get(format!("{}/users/abc", ctx.http_url()));
                    request = request.header("Host", "match-test.example.com");

                    match request.send().await {
                        Ok(response) => {
                            if response.status() == 404 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Regex path match works correctly (matched numbers, rejected letters)".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 404 for /users/abc, got {}", response.status()),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Negative test request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test Header Exact match
    fn test_header_exact_match() -> TestCase {
        TestCase::new("header_exact_match", "Test Header Exact match", |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();

                // Positive: with correct header value
                let mut request = ctx.http_client.get(format!("{}/header-test", ctx.http_url()));
                request = request.header("Host", "match-test.example.com");
                request = request.header("X-Custom-Header", "CustomValue");

                match request.send().await {
                    Ok(response) => {
                        if !response.status().is_success() {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Header exact match failed: {}", response.status()),
                            );
                        }
                    }
                    Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                }

                // Negative: header value mismatch
                let mut request = ctx.http_client.get(format!("{}/header-test", ctx.http_url()));
                request = request.header("Host", "match-test.example.com");
                request = request.header("X-Custom-Header", "WrongValue");

                match request.send().await {
                    Ok(response) => {
                        if response.status() == 404 {
                            TestResult::passed_with_message(
                                start.elapsed(),
                                "Header exact match works correctly (matched correct value, rejected wrong value)"
                                    .to_string(),
                            )
                        } else {
                            TestResult::failed(
                                start.elapsed(),
                                format!("Expected 404 for wrong header value, got {}", response.status()),
                            )
                        }
                    }
                    Err(e) => TestResult::failed(start.elapsed(), format!("Negative test request failed: {}", e)),
                }
            })
        })
    }

    /// Test Header Regex match
    fn test_header_regex_match() -> TestCase {
        TestCase::new("header_regex_match", "Test Header Regex match", |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();

                // Positive: header value matches regex ^v[0-9]+\.[0-9]+$
                let mut request = ctx.http_client.get(format!("{}/header-regex", ctx.http_url()));
                request = request.header("Host", "match-test.example.com");
                request = request.header("X-Version", "v1.2");

                match request.send().await {
                    Ok(response) => {
                        if !response.status().is_success() {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Header regex match failed for v1.2: {}", response.status()),
                            );
                        }
                    }
                    Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                }

                // Negative: header value does not match regex
                let mut request = ctx.http_client.get(format!("{}/header-regex", ctx.http_url()));
                request = request.header("Host", "match-test.example.com");
                request = request.header("X-Version", "invalid");

                match request.send().await {
                    Ok(response) => {
                        if response.status() == 404 {
                            TestResult::passed_with_message(
                                start.elapsed(),
                                "Header regex match works correctly (matched v1.2, rejected invalid)".to_string(),
                            )
                        } else {
                            TestResult::failed(
                                start.elapsed(),
                                format!("Expected 404 for invalid header value, got {}", response.status()),
                            )
                        }
                    }
                    Err(e) => TestResult::failed(start.elapsed(), format!("Negative test request failed: {}", e)),
                }
            })
        })
    }

    /// Test Query Parameter match
    fn test_query_param_match() -> TestCase {
        TestCase::new("query_param_match", "Test Query Parameter match", |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();

                // Positive: both query params match
                let mut request = ctx
                    .http_client
                    .get(format!("{}/query-test?apikey=secret123&version=10", ctx.http_url()));
                request = request.header("Host", "match-test.example.com");

                match request.send().await {
                    Ok(response) => {
                        if !response.status().is_success() {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Query param match failed: {}", response.status()),
                            );
                        }
                    }
                    Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                }

                // Negative: version param does not match the digits-only regex
                let mut request = ctx
                    .http_client
                    .get(format!("{}/query-test?apikey=secret123&version=abc", ctx.http_url()));
                request = request.header("Host", "match-test.example.com");

                match request.send().await {
                    Ok(response) => {
                        if response.status() == 404 {
                            TestResult::passed_with_message(
                                start.elapsed(),
                                "Query param match works correctly (matched valid params, rejected invalid)"
                                    .to_string(),
                            )
                        } else {
                            TestResult::failed(
                                start.elapsed(),
                                format!("Expected 404 for invalid query param, got {}", response.status()),
                            )
                        }
                    }
                    Err(e) => TestResult::failed(start.elapsed(), format!("Negative test request failed: {}", e)),
                }
            })
        })
    }

    /// Percent-encoded query parameter consistency:
    /// route matcher sees the decoded `hello world`, request must succeed.
    fn test_percent_encoded_query_param() -> TestCase {
        TestCase::new(
            "percent_encoded_query_param",
            "Percent-encoded query value matches decoded HTTPRoute queryParams",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Positive: `hello%20world` decodes to `hello world`.
                    // HTTPRoute Rule 9 declares queryParams.name = "hello world"
                    // (decoded form) — request matches only if the matcher
                    // percent-decodes before comparing.
                    let mut request = ctx
                        .http_client
                        .get(format!("{}/percent-query?name=hello%20world", ctx.http_url()));
                    request = request.header("Host", "match-test.example.com");

                    match request.send().await {
                        Ok(response) => {
                            if !response.status().is_success() {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Positive case: expected 2xx, got {}", response.status()),
                                );
                            }
                        }
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Positive request failed: {}", e));
                        }
                    }

                    // Negative: a different value must NOT match Rule 9, which
                    // expects the decoded `hello world`. Guards against a
                    // regression where the matcher accepts any /percent-query
                    // request regardless of the queryParams comparison.
                    let mut request = ctx
                        .http_client
                        .get(format!("{}/percent-query?name=foo", ctx.http_url()));
                    request = request.header("Host", "match-test.example.com");

                    match request.send().await {
                        Ok(response) => {
                            if response.status() == 404 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Percent-encoded query consistency holds (positive matched, negative rejected)"
                                        .to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Negative case: expected 404 for unmatched query, got {}",
                                        response.status()
                                    ),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Negative request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test HTTP Method match
    fn test_method_match() -> TestCase {
        TestCase::new("method_match", "Test HTTP Method match", |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();

                // Positive: POST /echo should match
                let mut request = ctx.http_client.post(format!("{}/echo", ctx.http_url()));
                request = request.header("Host", "match-test.example.com");
                request = request.body("test"); // Add body for POST request

                match request.send().await {
                    Ok(response) => {
                        if !response.status().is_success() {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("POST method match failed: {}", response.status()),
                            );
                        }
                    }
                    Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                }

                // No clean negative path: GET /echo matches other rules without a method constraint.
                TestResult::passed_with_message(
                    start.elapsed(),
                    "HTTP method match works correctly (POST /echo matched)".to_string(),
                )
            })
        })
    }

    /// Test combined match (path + method + headers + query params)
    fn test_combined_match() -> TestCase {
        TestCase::new(
            "combined_match",
            "Test combined match（path+method+headers+query params）",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Positive: POST /echo?action=update with specific headers
                    let mut request = ctx.http_client.post(format!("{}/echo?action=update", ctx.http_url()));
                    request = request.header("Host", "match-test.example.com");
                    request = request.header("Content-Type", "application/json");
                    request = request.header("X-Request-ID", "550e8400-e29b-41d4-a716-446655440000");
                    request = request.body("{}"); // Add body for POST request

                    match request.send().await {
                        Ok(response) => {
                            if !response.status().is_success() {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Combined match failed: {}", response.status()),
                                );
                            }
                        }
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }

                    // Negative: without ?action=update the query constraint on rule 8 is unmet
                    let mut request = ctx.http_client.post(format!("{}/echo", ctx.http_url()));
                    request = request.header("Host", "match-test.example.com");
                    request = request.header("Content-Type", "application/json");
                    request = request.header("X-Request-ID", "550e8400-e29b-41d4-a716-446655440000");
                    request = request.body("{}");

                    match request.send().await {
                        Ok(response) => {
                            // Rule 8 requires ?action=update; without it, rule 7 (POST /echo, no
                            // query constraint) matches instead, returning 200.
                            if response.status().is_success() {
                                TestResult::passed_with_message(
                                start.elapsed(),
                                "Combined match works correctly (matched rule 7, not rule 8 due to missing query param)".to_string()
                            )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200 (match rule 7), got {}", response.status()),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Negative test request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test SectionName match (parentRefs with sectionName binding)
    fn test_section_name_match() -> TestCase {
        TestCase::new(
            "section_name_match",
            "Test SectionName match（bound to specific listener）",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Positive: access via HTTP listener (sectionName: http)
                    let mut request = ctx.http_client.get(format!("{}/health", ctx.http_url()));
                    request = request.header("Host", "section-test.example.com");

                    match request.send().await {
                        Ok(response) => {
                            if !response.status().is_success() {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("SectionName match failed via HTTP listener: {}", response.status()),
                                );
                            }
                        }
                        Err(e) => return TestResult::failed(start.elapsed(), format!("HTTP request failed: {}", e)),
                    }

                    // Negative case not exercised here: this domain is bound to sectionName: http,
                    // so the HTTPS listener does not serve it. Without a cert loaded for the domain,
                    // TLS handshake fails rather than returning 404, confirming listener isolation.

                    TestResult::passed_with_message(
                        start.elapsed(),
                        "SectionName match works correctly (successfully matched HTTP listener with sectionName: http)"
                            .to_string(),
                    )
                })
            },
        )
    }

    /// Test wildcard hostname matching (*.wc-match-test.example.com)
    fn test_wildcard_hostname_match() -> TestCase {
        TestCase::new(
            "wildcard_hostname_match",
            "Test wildcard hostname matching with *.wc-match-test.example.com",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Test 1: Single-level subdomain should match
                    let mut request1 = ctx.http_client.get(format!("{}/wildcard-test", ctx.http_url()));
                    request1 = request1.header("Host", "api.wc-match-test.example.com");

                    match request1.send().await {
                        Ok(response) => {
                            if !response.status().is_success() {
                                return TestResult::failed(
                                start.elapsed(),
                                format!("Single-level subdomain (api.wc-match-test.example.com) should match *.wc-match-test.example.com, got status: {}", response.status())
                            );
                            }
                        }
                        Err(e) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Request to api.wc-match-test.example.com failed: {}", e),
                            );
                        }
                    }

                    // Test 2: Multi-level subdomain should also match (per Gateway API spec, suffix match)
                    let mut request2 = ctx.http_client.get(format!("{}/wildcard-test", ctx.http_url()));
                    request2 = request2.header("Host", "foo.bar.wc-match-test.example.com");

                    match request2.send().await {
                        Ok(response) => {
                            if !response.status().is_success() {
                                return TestResult::failed(
                                start.elapsed(),
                                format!("Multi-level subdomain (foo.bar.wc-match-test.example.com) should match *.wc-match-test.example.com per Gateway API spec, got status: {}", response.status())
                            );
                            }
                        }
                        Err(e) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Request to foo.bar.wc-match-test.example.com failed: {}", e),
                            );
                        }
                    }

                    // Test 3: Root domain should NOT match
                    let mut request3 = ctx.http_client.get(format!("{}/wildcard-test", ctx.http_url()));
                    request3 = request3.header("Host", "wc-match-test.example.com");

                    match request3.send().await {
                        Ok(response) => {
                            if response.status().is_success() {
                                return TestResult::failed(
                                start.elapsed(),
                                format!("Root domain (wc-match-test.example.com) should NOT match *.wc-match-test.example.com, but got success status: {}", response.status())
                            );
                            }
                        }
                        Err(_e) => {}
                    }

                    TestResult::passed_with_message(
                    start.elapsed(),
                    "Wildcard hostname matching works correctly: *.wc-match-test.example.com matches single and multi-level subdomains, but NOT root domain".to_string()
                )
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for HttpMatchTestSuite {
    fn name(&self) -> &str {
        "HTTP Match Rules"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_path_prefix_match(),
            Self::test_exact_path_match(),
            Self::test_regex_path_match(),
            Self::test_header_exact_match(),
            Self::test_header_regex_match(),
            Self::test_query_param_match(),
            Self::test_percent_encoded_query_param(),
            Self::test_method_match(),
            Self::test_combined_match(),
            Self::test_section_name_match(),
            Self::test_wildcard_hostname_match(),
        ]
    }
}
