// Key Auth Plugin Test Suite
//
// Required config files (in edgion-tests/integration/conf/EdgionPlugins/KeyAuth/):
// - 01_Secret_default_api-keys.yaml                       # API keys with metadata
// - EdgionPlugins_default_key-auth.yaml                   # KeyAuth plugin with header/query/cookie
// - HTTPRoute_default_key-auth-test.yaml                  # Route with host: key-auth-test.example.com
// - 02_EdgionPlugins_default_key-auth-anonymous.yaml      # KeyAuth with anonymous access
// - 03_HTTPRoute_default_key-auth-anonymous.yaml          # Route for anonymous test
// - 04_EdgionPlugins_default_key-auth-hide-creds.yaml     # KeyAuth with hideCredentials
// - 05_HTTPRoute_default_key-auth-hide-creds.yaml         # Route for hideCredentials test
// - 08_EdgionPlugins_default_key-auth-dual-alias.yaml     # Two KeyAuth entries with distinct aliases
// - 09_HTTPRoute_default_key-auth-dual-alias.yaml         # Route for dual-alias test
// - 10_Secret_default_api-keys-grouped.yaml               # Two grouped Secrets
// - 11_EdgionPlugins_default_key-auth-grouped.yaml        # KeyAuth with partner-a + partner-b
// - 12_HTTPRoute_default_key-auth-grouped.yaml            # Route for grouped test
// - 13_EdgionPlugins_default_key-auth-anon-only.yaml      # anonymous-only (no secretGroups)
// - 14_HTTPRoute_default_key-auth-anon-only.yaml          # Route for anon-only test
//
// Also requires base config (in edgion-tests/integration/conf/EdgionPlugins/base/):
// - Gateway.yaml                                          # Gateway for EdgionPlugins tests

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

pub struct KeyAuthTestSuite;

/// Test API keys (must match 01_Secret_default_api-keys.yaml)
const VALID_KEY_JACK: &str = "test-key-jack-12345";
const VALID_KEY_ALICE: &str = "test-key-alice-67890";
const INVALID_KEY: &str = "invalid-api-key-xxxxx";

/// Test hosts
const TEST_HOST: &str = "key-auth-test.example.com";
const ANONYMOUS_HOST: &str = "key-auth-anonymous.example.com";
const HIDE_CREDS_HOST: &str = "key-auth-hide-creds.example.com";
const DELAY_HOST: &str = "key-auth-delay.example.com";
const DUAL_ALIAS_HOST: &str = "key-auth-dual-alias.example.com";

/// Test hosts for grouped and anon-only cases
const GROUPED_HOST: &str = "key-auth-grouped.example.com";
const GROUPED_KEY_PARTNER_A: &str = "grouped-key-partner-a-001";
const GROUPED_KEY_PARTNER_B: &str = "grouped-key-partner-b-001";
const ANON_ONLY_HOST: &str = "key-auth-anon-only.example.com";

/// authFailureDelayMs configured in 06_EdgionPlugins_default_key-auth-delay.yaml
const CONFIGURED_DELAY_MS: u64 = 300;
/// Tolerance: allow 50ms below the configured value
const DELAY_MIN_MS: u64 = CONFIGURED_DELAY_MS - 50;

fn validate_anonymous_identity(body: &str, anonymous: bool) -> Result<(), &'static str> {
    let response: serde_json::Value = serde_json::from_str(body).map_err(|_| "Invalid identity echo JSON")?;
    let headers = response
        .get("headers")
        .and_then(|value| value.as_object())
        .ok_or("Missing identity echo headers object")?;
    let expected = if anonymous { "anonymous-user" } else { "jack" };
    if headers.get("x-consumer-username").and_then(|value| value.as_str()) != Some(expected) {
        return Err("Missing or mismatched x-consumer-username");
    }
    let marker = match headers.get("x-anonymous-consumer") {
        None => None,
        Some(value) => Some(value.as_str().ok_or("Invalid x-anonymous-consumer value")?),
    };
    if anonymous && marker != Some("true") {
        return Err("Missing or mismatched x-anonymous-consumer");
    }
    if !anonymous && marker.is_some_and(|value| value.eq_ignore_ascii_case("true")) {
        return Err("Authenticated identity has affirmative x-anonymous-consumer");
    }
    Ok(())
}

async fn observe_hidden_key_response(response: reqwest::Response) -> Result<(), &'static str> {
    if response.status().as_u16() != 200 {
        return Err("Expected HTTP 200 for the hidden-credential header observation");
    }
    let body = response
        .text()
        .await
        .map_err(|_| "Hidden-credential response body read failed")?;
    let echoed: serde_json::Value =
        serde_json::from_str(&body).map_err(|_| "Hidden-credential response was not valid headers JSON")?;
    if echoed.get("headers").and_then(serde_json::Value::as_object).is_none() {
        return Err("Hidden-credential response did not contain an object-valued headers member");
    }
    let body_lower = body.to_lowercase();
    if body_lower.contains("x-api-key") && body_lower.contains("test-key-jack") {
        return Err("X-API-Key was NOT removed from request");
    }
    Ok(())
}

impl KeyAuthTestSuite {
    /// Test: Valid API key in header should return 200
    fn test_valid_key_in_header() -> TestCase {
        TestCase::new(
            "valid_key_in_header",
            "Valid API key in X-API-Key header returns 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/health", ctx.http_url());

                    let request = client
                        .get(&url)
                        .header("host", TEST_HOST)
                        .header("X-API-Key", VALID_KEY_JACK);

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Valid API key accepted, returned 200".to_string(),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test: No API key should return 401
    fn test_no_key_returns_401() -> TestCase {
        TestCase::new(
            "no_key_returns_401",
            "Request without API key returns 401",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/health", ctx.http_url());

                    let request = client.get(&url).header("host", TEST_HOST);

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 401 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "No key returns 401 as expected".to_string(),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected 401, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test: Invalid API key should return 401
    fn test_invalid_key_returns_401() -> TestCase {
        TestCase::new(
            "invalid_key_returns_401",
            "Invalid API key returns 401",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/health", ctx.http_url());

                    let request = client
                        .get(&url)
                        .header("host", TEST_HOST)
                        .header("X-API-Key", INVALID_KEY);

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 401 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Invalid key returns 401 as expected".to_string(),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected 401, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test: API key in query parameter should work
    fn test_key_in_query() -> TestCase {
        TestCase::new(
            "key_in_query",
            "Valid API key in query parameter returns 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/health?api_key={}", ctx.http_url(), VALID_KEY_ALICE);

                    let request = client.get(&url).header("host", TEST_HOST);

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "API key in query accepted, returned 200".to_string(),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test: API key in cookie should work
    fn test_key_in_cookie() -> TestCase {
        TestCase::new(
            "key_in_cookie",
            "Valid API key in cookie returns 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/health", ctx.http_url());

                    let request = client
                        .get(&url)
                        .header("host", TEST_HOST)
                        .header("Cookie", format!("api_key={}", VALID_KEY_JACK));

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "API key in cookie accepted, returned 200".to_string(),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test: 401 response should include WWW-Authenticate header
    fn test_www_authenticate_header() -> TestCase {
        TestCase::new(
            "www_authenticate_header",
            "401 response includes WWW-Authenticate: ApiKey header",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/health", ctx.http_url());

                    let request = client.get(&url).header("host", TEST_HOST);

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status != 401 {
                                return TestResult::failed(start.elapsed(), format!("Expected 401, got {}", status));
                            }

                            // Check for WWW-Authenticate header
                            let www_auth = response.headers().get("WWW-Authenticate").and_then(|v| v.to_str().ok());

                            match www_auth {
                                Some(value) if value.starts_with("ApiKey") => TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Got WWW-Authenticate: {}", value),
                                ),
                                Some(value) => TestResult::failed(
                                    start.elapsed(),
                                    format!("WWW-Authenticate header exists but unexpected value: {}", value),
                                ),
                                None => TestResult::failed(
                                    start.elapsed(),
                                    "Missing WWW-Authenticate header in 401 response".to_string(),
                                ),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test: Header takes priority over query/cookie
    fn test_key_extraction_priority() -> TestCase {
        TestCase::new(
            "key_extraction_priority",
            "Header key takes priority over query/cookie",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;

                    // Valid key in header, invalid in query and cookie
                    let url = format!("{}/health?api_key={}", ctx.http_url(), INVALID_KEY);

                    let request = client
                        .get(&url)
                        .header("host", TEST_HOST)
                        .header("X-API-Key", VALID_KEY_JACK)
                        .header("Cookie", format!("api_key={}", INVALID_KEY));

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Header key took priority, request succeeded".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200 (header key should be used), got {}", status),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test: Different keys work for different users
    fn test_multiple_keys() -> TestCase {
        TestCase::new(
            "multiple_keys",
            "Different API keys are all valid",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/health", ctx.http_url());

                    // Test Jack's key
                    let request1 = client
                        .get(&url)
                        .header("host", TEST_HOST)
                        .header("X-API-Key", VALID_KEY_JACK);

                    let status1 = match request1.send().await {
                        Ok(response) => response.status().as_u16(),
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Jack's request failed: {}", e)),
                    };

                    if status1 != 200 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Jack's key failed: expected 200, got {}", status1),
                        );
                    }

                    // Test Alice's key
                    let request2 = client
                        .get(&url)
                        .header("host", TEST_HOST)
                        .header("X-API-Key", VALID_KEY_ALICE);

                    let status2 = match request2.send().await {
                        Ok(response) => response.status().as_u16(),
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Alice's request failed: {}", e)),
                    };

                    if status2 != 200 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Alice's key failed: expected 200, got {}", status2),
                        );
                    }

                    TestResult::passed_with_message(start.elapsed(), "Both Jack's and Alice's keys worked".to_string())
                })
            },
        )
    }

    // ==========================================
    // Anonymous Access Tests
    // ==========================================

    /// Test: Anonymous access allowed when no key provided (anonymous config)
    fn test_anonymous_access_no_key() -> TestCase {
        TestCase::new(
            "anonymous_access_no_key",
            "Anonymous access allowed when no key provided",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/headers", ctx.http_url());

                    // No API key, should be allowed with anonymous user
                    let request = client.get(&url).header("host", ANONYMOUS_HOST);

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200 for anonymous access, got {}", status),
                                );
                            }

                            let body = match response.text().await {
                                Ok(body) => body,
                                Err(_) => {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        "Failed to read anonymous identity echo".to_string(),
                                    )
                                }
                            };
                            match validate_anonymous_identity(&body, true) {
                                Ok(()) => TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Anonymous username and marker observed".to_string(),
                                ),
                                Err(error) => TestResult::failed(start.elapsed(), error.to_string()),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test: Valid key still works with anonymous config
    fn test_anonymous_with_valid_key() -> TestCase {
        TestCase::new(
            "anonymous_with_valid_key",
            "Valid API key works even with anonymous enabled",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/headers", ctx.http_url());

                    let request = client
                        .get(&url)
                        .header("host", ANONYMOUS_HOST)
                        .header("X-API-Key", VALID_KEY_JACK);

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 200 {
                                let body = match response.text().await {
                                    Ok(body) => body,
                                    Err(_) => {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            "Failed to read authenticated identity echo".to_string(),
                                        )
                                    }
                                };
                                match validate_anonymous_identity(&body, false) {
                                    Ok(()) => TestResult::passed_with_message(
                                        start.elapsed(),
                                        "Authenticated username observed without anonymous marker".to_string(),
                                    ),
                                    Err(error) => TestResult::failed(start.elapsed(), error.to_string()),
                                }
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==========================================
    // Hide Credentials Tests
    // ==========================================

    /// Test: API key header is removed when hideCredentials is true
    fn test_hide_credentials_header_removed() -> TestCase {
        TestCase::new(
            "hide_credentials_header_removed",
            "X-API-Key header is removed from upstream request when hideCredentials=true",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    // Use /headers endpoint which echoes back received headers
                    let url = format!("{}/headers", ctx.http_url());

                    let request = client
                        .get(&url)
                        .header("host", HIDE_CREDS_HOST)
                        .header("X-API-Key", VALID_KEY_JACK);

                    match request.send().await {
                        Ok(response) => match observe_hidden_key_response(response).await {
                            Ok(()) => TestResult::passed_with_message(
                                start.elapsed(),
                                "X-API-Key header was removed from upstream request".to_string(),
                            ),
                            Err(message) => TestResult::failed(start.elapsed(), message.to_string()),
                        },
                        Err(_) => TestResult::failed(start.elapsed(), "Hidden-credential request failed".to_string()),
                    }
                })
            },
        )
    }

    // ==========================================
    // Upstream Headers Tests
    // ==========================================

    /// Test: Consumer metadata headers are forwarded to upstream
    fn test_upstream_headers_forwarded() -> TestCase {
        TestCase::new(
            "upstream_headers_forwarded",
            "Consumer metadata headers (X-Consumer-Username, X-Customer-ID) are forwarded",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    // Use /headers endpoint which echoes back received headers
                    let url = format!("{}/headers", ctx.http_url());

                    let request = client
                        .get(&url)
                        .header("host", TEST_HOST)
                        .header("X-API-Key", VALID_KEY_JACK);

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                            }

                            let body = response.text().await.unwrap_or_default();
                            let body_lower = body.to_lowercase();

                            let mut messages = Vec::new();
                            let mut missing = Vec::new();

                            // Check for X-Consumer-Username: jack
                            if body_lower.contains("x-consumer-username") && body_lower.contains("jack") {
                                messages.push("X-Consumer-Username: jack");
                            } else {
                                missing.push("X-Consumer-Username");
                            }

                            // Check for X-Customer-ID: cust-001
                            if body_lower.contains("x-customer-id") && body_lower.contains("cust-001") {
                                messages.push("X-Customer-ID: cust-001");
                            } else {
                                missing.push("X-Customer-ID");
                            }

                            // Check for X-User-Tier: premium
                            if body_lower.contains("x-user-tier") && body_lower.contains("premium") {
                                messages.push("X-User-Tier: premium");
                            } else {
                                missing.push("X-User-Tier");
                            }

                            if missing.is_empty() {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("All headers forwarded: {}", messages.join(", ")),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Missing headers: {}. Found: {}. Body: {}",
                                        missing.join(", "),
                                        messages.join(", "),
                                        body
                                    ),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test: Alice's headers are correctly forwarded
    fn test_upstream_headers_alice() -> TestCase {
        TestCase::new(
            "upstream_headers_alice",
            "Alice's metadata headers are correctly forwarded",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/headers", ctx.http_url());

                    let request = client
                        .get(&url)
                        .header("host", TEST_HOST)
                        .header("X-API-Key", VALID_KEY_ALICE);

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                            }

                            let body = response.text().await.unwrap_or_default();
                            let body_lower = body.to_lowercase();

                            // Check for Alice's data
                            let has_username = body_lower.contains("alice");
                            let has_customer_id = body_lower.contains("cust-002");
                            let has_tier = body_lower.contains("basic");

                            if has_username && has_customer_id && has_tier {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Alice's headers: alice, cust-002, basic".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Missing Alice's data. username={}, customer_id={}, tier={}. Body: {}",
                                        has_username, has_customer_id, has_tier, body
                                    ),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==========================================
    // auth_failure_delay_ms Tests
    // ==========================================

    /// Test: Missing key with delay — 401 must arrive after at least DELAY_MIN_MS
    fn test_failure_delay_on_missing_key() -> TestCase {
        TestCase::new(
            "failure_delay_on_missing_key",
            "401 (no key) is delayed by authFailureDelayMs",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/health", ctx.http_url());

                    let resp = match ctx.http_client.get(&url).header("host", DELAY_HOST).send().await {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };

                    let elapsed = start.elapsed();
                    let status = resp.status().as_u16();

                    if status != 401 {
                        return TestResult::failed(elapsed, format!("Expected 401 (no key), got {}", status));
                    }

                    let elapsed_ms = elapsed.as_millis() as u64;
                    if elapsed_ms >= DELAY_MIN_MS {
                        TestResult::passed_with_message(
                            elapsed,
                            format!(
                                "No-key 401 delayed {}ms (>= {}ms threshold, configured {}ms)",
                                elapsed_ms, DELAY_MIN_MS, CONFIGURED_DELAY_MS
                            ),
                        )
                    } else {
                        TestResult::failed(
                            elapsed,
                            format!(
                                "Delay too short: {}ms < {}ms (configured authFailureDelayMs={}ms)",
                                elapsed_ms, DELAY_MIN_MS, CONFIGURED_DELAY_MS
                            ),
                        )
                    }
                })
            },
        )
    }

    /// Test: Invalid key with delay — 401 must also be delayed
    fn test_failure_delay_on_invalid_key() -> TestCase {
        TestCase::new(
            "failure_delay_on_invalid_key",
            "401 (invalid key) is delayed by authFailureDelayMs",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/health", ctx.http_url());

                    let resp = match ctx
                        .http_client
                        .get(&url)
                        .header("host", DELAY_HOST)
                        .header("X-API-Key", INVALID_KEY)
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };

                    let elapsed = start.elapsed();
                    let status = resp.status().as_u16();

                    if status != 401 {
                        return TestResult::failed(elapsed, format!("Expected 401 (invalid key), got {}", status));
                    }

                    let elapsed_ms = elapsed.as_millis() as u64;
                    if elapsed_ms >= DELAY_MIN_MS {
                        TestResult::passed_with_message(
                            elapsed,
                            format!("Invalid-key 401 delayed {}ms (>= {}ms)", elapsed_ms, DELAY_MIN_MS),
                        )
                    } else {
                        TestResult::failed(
                            elapsed,
                            format!("Delay too short: {}ms < {}ms", elapsed_ms, DELAY_MIN_MS),
                        )
                    }
                })
            },
        )
    }

    /// Test: Valid key with delay config must NOT be delayed
    fn test_no_delay_on_valid_key() -> TestCase {
        TestCase::new(
            "no_delay_on_valid_key",
            "Successful auth is NOT delayed even when authFailureDelayMs is configured",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/health", ctx.http_url());

                    let resp = match ctx
                        .http_client
                        .get(&url)
                        .header("host", DELAY_HOST)
                        .header("X-API-Key", VALID_KEY_JACK)
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };

                    let elapsed = start.elapsed();
                    let status = resp.status().as_u16();

                    if status != 200 {
                        return TestResult::failed(
                            elapsed,
                            format!("Expected 200 for valid key on delay host, got {}", status),
                        );
                    }

                    let upper_bound_ms = CONFIGURED_DELAY_MS * 2;
                    let elapsed_ms = elapsed.as_millis() as u64;

                    if elapsed_ms < upper_bound_ms {
                        TestResult::passed_with_message(
                            elapsed,
                            format!("Valid key completed in {}ms — no unwanted delay", elapsed_ms),
                        )
                    } else {
                        // Soft warn: CI can be slow
                        TestResult::passed_with_message(
                            elapsed,
                            format!(
                                "Warning: valid key took {}ms (>= {}ms upper bound) — possible CI slowness",
                                elapsed_ms, upper_bound_ms
                            ),
                        )
                    }
                })
            },
        )
    }

    // ==========================================
    // Dual-alias Access Log Tests
    // ==========================================

    /// Test: Two KeyAuth entries with distinct aliases both appear in access log stage_logs.
    ///
    /// Requires:
    ///  - 08_EdgionPlugins_default_key-auth-dual-alias.yaml  (two KeyAuth entries with aliases)
    ///  - 09_HTTPRoute_default_key-auth-dual-alias.yaml       (host: key-auth-dual-alias.example.com)
    ///  - Gateway started with --integration-testing-mode
    fn test_dual_alias_in_access_log() -> TestCase {
        TestCase::new(
            "dual_alias_in_access_log",
            "Two same-type KeyAuth entries with distinct aliases both appear in access log",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = format!("ka-dual-alias-{}", uuid::Uuid::new_v4());
                    let url = format!("{}/health", ctx.http_url());

                    // Send a request that satisfies both KeyAuth entries (valid key in X-API-Key).
                    // Also include access_log: test_store to ensure the entry is stored.
                    let resp = match ctx
                        .http_client
                        .get(&url)
                        .header("host", DUAL_ALIAS_HOST)
                        .header("X-API-Key", VALID_KEY_JACK)
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker(&trace_id),
                        )
                        .header("access_log", "test_store")
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };

                    let status = resp.status().as_u16();
                    // Consume body to ensure the request is fully flushed
                    let _ = resp.text().await;

                    if status != 200 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Expected 200 (valid key, dual-alias route), got {}", status),
                        );
                    }

                    // Fetch the access log from the store with retry
                    let al_client = ctx.access_log_client();
                    let entry = match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                        Ok(e) => e,
                        Err(e) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Failed to fetch access log for trace_id {}: {}", trace_id, e),
                            )
                        }
                    };

                    // Walk stage_logs[].edgion_plugins[].logs[] looking for two KeyAuth entries
                    // with distinct alias values
                    let mut found_aliases: Vec<String> = Vec::new();
                    if let Some(stages) = entry.data["stage_logs"].as_array() {
                        for stage in stages {
                            if let Some(eps) = stage["edgion_plugins"].as_array() {
                                for ep in eps {
                                    if let Some(logs) = ep["logs"].as_array() {
                                        for log in logs {
                                            if log["name"].as_str() == Some("KeyAuth") {
                                                if let Some(alias) = log["alias"].as_str() {
                                                    found_aliases.push(alias.to_string());
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    if found_aliases.len() != 2 {
                        let raw = serde_json::to_string_pretty(&entry.data).unwrap_or_default();
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected exactly 2 KeyAuth entries with alias in stage_logs[].edgion_plugins[].logs[], got {}. Found aliases: {:?}. Raw access log: {}",
                                found_aliases.len(),
                                found_aliases,
                                &raw[..raw.len().min(1500)]
                            ),
                        );
                    }

                    let has_primary = found_aliases.iter().any(|a| a == "auth-primary");
                    let has_secondary = found_aliases.iter().any(|a| a == "auth-secondary");
                    if !has_primary || !has_secondary {
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected aliases auth-primary and auth-secondary; found: {:?}",
                                found_aliases
                            ),
                        );
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        "Both KeyAuth aliases (auth-primary, auth-secondary) found in access log".to_string(),
                    )
                })
            },
        )
    }

    // ==========================================
    // Grouped Secret Tests
    // ==========================================

    /// Test: Key from partner-a group returns 200
    fn test_grouped_partner_a_returns_200() -> TestCase {
        TestCase::new(
            "grouped_partner_a_returns_200",
            "partner-a group key accepted, returns 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/health", ctx.http_url());

                    let request = client
                        .get(&url)
                        .header("host", GROUPED_HOST)
                        .header("X-API-Key", GROUPED_KEY_PARTNER_A);

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "partner-a key accepted, returned 200".to_string(),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test: Key from partner-b group returns 200
    fn test_grouped_partner_b_returns_200() -> TestCase {
        TestCase::new(
            "grouped_partner_b_returns_200",
            "partner-b group key accepted, returns 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/health", ctx.http_url());

                    let request = client
                        .get(&url)
                        .header("host", GROUPED_HOST)
                        .header("X-API-Key", GROUPED_KEY_PARTNER_B);

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "partner-b key accepted, returned 200".to_string(),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test: Request without key to anon-only route returns 200 (anonymous path)
    fn test_anon_only_no_key_returns_200() -> TestCase {
        TestCase::new(
            "anon_only_no_key_returns_200",
            "Anonymous-only route returns 200 when no key is provided",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/health", ctx.http_url());

                    let request = client.get(&url).header("host", ANON_ONLY_HOST);

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Anonymous access granted (200 OK), secretGroups absent".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200 for anon-only, got {}", status),
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

impl TestSuite for KeyAuthTestSuite {
    fn name(&self) -> &str {
        "KeyAuth"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            // Basic tests
            Self::test_valid_key_in_header(),
            Self::test_no_key_returns_401(),
            Self::test_invalid_key_returns_401(),
            // Key extraction from different sources
            Self::test_key_in_query(),
            Self::test_key_in_cookie(),
            Self::test_key_extraction_priority(),
            // Multiple keys test
            Self::test_multiple_keys(),
            // WWW-Authenticate header
            Self::test_www_authenticate_header(),
            // Anonymous access tests
            Self::test_anonymous_access_no_key(),
            Self::test_anonymous_with_valid_key(),
            // Hide credentials test
            Self::test_hide_credentials_header_removed(),
            // Upstream headers tests
            Self::test_upstream_headers_forwarded(),
            Self::test_upstream_headers_alice(),
            // auth_failure_delay_ms tests
            Self::test_failure_delay_on_missing_key(),
            Self::test_failure_delay_on_invalid_key(),
            Self::test_no_delay_on_valid_key(),
            // Dual-alias access log test
            Self::test_dual_alias_in_access_log(),
            // Grouped secret tests
            Self::test_grouped_partner_a_returns_200(),
            Self::test_grouped_partner_b_returns_200(),
            // Anon-only test (secretGroups absent)
            Self::test_anon_only_no_key_returns_200(),
        ]
    }
}

#[cfg(test)]
mod anonymous_identity_tests {
    use super::validate_anonymous_identity;
    use serde_json::json;

    #[test]
    fn anonymous_and_authenticated_echoes_require_distinct_identities() {
        assert!(validate_anonymous_identity(
            r#"{"headers":{"x-consumer-username":"anonymous-user","x-anonymous-consumer":"true"}}"#,
            true
        )
        .is_ok());
        for marker in [None, Some("false")] {
            let mut value = json!({"headers": {"x-consumer-username": "jack"}});
            if let Some(marker) = marker {
                value["headers"]["x-anonymous-consumer"] = json!(marker);
            }
            assert!(validate_anonymous_identity(&value.to_string(), false).is_ok());
        }
        for marker in [json!("true"), json!("TRUE"), json!(true), json!(null)] {
            let value = json!({"headers":{"x-consumer-username":"jack","x-anonymous-consumer":marker}});
            assert!(validate_anonymous_identity(&value.to_string(), false).is_err());
        }
        assert!(validate_anonymous_identity(
            r#"{"headers":{"x-consumer-username":"anonymous-user","x-anonymous-consumer":"true"}}"#,
            false
        )
        .is_err());
        assert!(validate_anonymous_identity(r#"{"headers":{"x-consumer-username":"anonymous-user"}}"#, true).is_err());
    }

    #[test]
    fn identity_echo_errors_are_safe_and_fail_closed() {
        for input in [
            "",
            "[]",
            "null",
            "{}",
            r#"{"headers":[]}"#,
            r#"{"headers":{}}"#,
            r#"{"headers":{"x-consumer-username":7}}"#,
            r#"{"headers":{"x-consumer-username":"sensitive-identity-sentinel","x-api-key":"sensitive-key-sentinel"}}"#,
        ] {
            for anonymous in [true, false] {
                let error = validate_anonymous_identity(input, anonymous).unwrap_err();
                assert!(!error.contains("sensitive-"));
            }
        }
    }
}

#[cfg(test)]
mod observation_tests {
    use super::observe_hidden_key_response;
    use bytes::Bytes;
    use futures::stream;
    use http::Response as HttpResponse;
    use reqwest::{Body, Response};
    use std::io;

    fn response(status: u16, body: &str) -> Response {
        HttpResponse::builder()
            .status(status)
            .body(body.to_string())
            .unwrap()
            .into()
    }

    fn interrupted_response(status: u16, prefix: &str) -> Response {
        let chunks = [
            Ok(Bytes::copy_from_slice(prefix.as_bytes())),
            Err(io::Error::other("sensitive-observation-sentinel")),
        ];
        HttpResponse::builder()
            .status(status)
            .body(Body::wrap_stream(stream::iter(chunks)))
            .unwrap()
            .into()
    }

    const HEADERS: &str = r#"{"headers":{"host":"key-auth-hide-creds.example.com","x-consumer-username":"jack"}}"#;

    #[tokio::test]
    async fn key_hidden_credentials_requires_an_observed_headers_object() {
        assert_eq!(observe_hidden_key_response(response(200, HEADERS)).await, Ok(()));
        for body in [
            "",
            "unrelated success",
            "{",
            "null",
            "[]",
            "{}",
            r#"{"headers":null}"#,
            r#"{"headers":[]}"#,
            r#"{"headers":"empty"}"#,
        ] {
            assert!(observe_hidden_key_response(response(200, body)).await.is_err());
        }
        assert!(observe_hidden_key_response(response(500, HEADERS)).await.is_err());
    }

    #[tokio::test]
    async fn key_hidden_credentials_preserves_the_two_marker_conjunction() {
        for body in [
            r#"{"headers":{"x-api-key":"other"}}"#,
            r#"{"headers":{"x-other":"test-key-jack"}}"#,
        ] {
            assert_eq!(observe_hidden_key_response(response(200, body)).await, Ok(()));
        }
        let body = r#"{"headers":{"X-API-Key":"test-key-jack sensitive-observation-sentinel"}}"#;
        assert_eq!(
            observe_hidden_key_response(response(200, body)).await,
            Err("X-API-Key was NOT removed from request"),
        );
    }

    #[tokio::test]
    async fn key_hidden_credentials_read_errors_fail_even_after_valid_header_bytes() {
        for prefix in ["", HEADERS] {
            assert_eq!(
                observe_hidden_key_response(interrupted_response(200, prefix)).await,
                Err("Hidden-credential response body read failed"),
            );
        }
    }
}
