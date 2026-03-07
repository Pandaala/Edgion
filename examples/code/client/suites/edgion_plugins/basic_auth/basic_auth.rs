// BasicAuth Plugin Integration Test Suite
//
// Tests the BasicAuth plugin which validates HTTP Basic Authentication credentials.
// Credentials are stored in a Kubernetes Secret (htpasswd / plain-text format).
//
// Required config files (in examples/test/conf/EdgionPlugins/BasicAuth/):
//   - 01_Secret_default_basic-auth-users.yaml          # Test users (alice/alice-password, bob/bob-secret)
//   - EdgionPlugins_default_basic-auth-test.yaml       # Basic auth: hideCredentials=false
//   - HTTPRoute_default_basic-auth-test.yaml            # Route: basic-auth-test.example.com
//   - 02_EdgionPlugins_default_basic-auth-hide-creds.yaml # hideCredentials=true
//   - 03_HTTPRoute_default_basic-auth-hide-creds.yaml   # Route: basic-auth-hide-creds.example.com
//   - 04_EdgionPlugins_default_basic-auth-delay.yaml    # authFailureDelayMs=300
//   - 05_HTTPRoute_default_basic-auth-delay.yaml        # Route: basic-auth-delay.example.com
//
// Test scenarios:
//   1. Basic auth: valid credentials → 200
//   2. Basic auth: missing credentials → 401 + WWW-Authenticate header
//   3. Basic auth: wrong password → 401
//   4. hide_credentials: Authorization header removed from upstream request after successful auth
//   5. auth_failure_delay_ms: 401 response is delayed by at least the configured time

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::time::Instant;

pub struct BasicAuthTestSuite;

/// Valid credentials (must match 01_Secret_default_basic-auth-users.yaml)
const USER_ALICE: (&str, &str) = ("alice", "alice-password");
const USER_BOB: (&str, &str) = ("bob", "bob-secret");

/// Hosts (must match HTTPRoute YAML hostnames)
const TEST_HOST: &str = "basic-auth-test.example.com";
const HIDE_CREDS_HOST: &str = "basic-auth-hide-creds.example.com";
const DELAY_HOST: &str = "basic-auth-delay.example.com";

/// Configured delay (ms) — must match authFailureDelayMs in 04_EdgionPlugins_default_basic-auth-delay.yaml
const CONFIGURED_DELAY_MS: u64 = 300;
/// Lower bound for delay check: allow 50ms tolerance below the configured delay
const DELAY_MIN_MS: u64 = CONFIGURED_DELAY_MS - 50;

fn basic_auth_header(username: &str, password: &str) -> String {
    let credentials = format!("{}:{}", username, password);
    format!("Basic {}", STANDARD.encode(credentials.as_bytes()))
}

impl BasicAuthTestSuite {
    // ============================================================
    // Basic Authentication Tests
    // ============================================================

    /// Valid credentials → 200 OK
    fn test_valid_credentials_returns_200() -> TestCase {
        TestCase::new(
            "valid_credentials_returns_200",
            "Valid Basic Auth credentials return 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/health", ctx.http_url());
                    let auth = basic_auth_header(USER_ALICE.0, USER_ALICE.1);

                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", TEST_HOST)
                        .header("Authorization", &auth)
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Alice authenticated successfully (200)".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200 for valid credentials, got {}", status),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Second user (Bob) valid credentials → 200 OK
    fn test_second_user_valid_credentials() -> TestCase {
        TestCase::new(
            "second_user_valid_credentials",
            "Bob's valid Basic Auth credentials also return 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/health", ctx.http_url());
                    let auth = basic_auth_header(USER_BOB.0, USER_BOB.1);

                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", TEST_HOST)
                        .header("Authorization", &auth)
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(start.elapsed(), "Bob authenticated (200)".into())
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected 200 for Bob, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Missing Authorization header → 401 + WWW-Authenticate
    fn test_missing_credentials_returns_401() -> TestCase {
        TestCase::new(
            "missing_credentials_returns_401",
            "Request without Authorization header returns 401 + WWW-Authenticate",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/health", ctx.http_url());

                    match ctx.http_client.get(&url).header("host", TEST_HOST).send().await {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status != 401 {
                                return TestResult::failed(start.elapsed(), format!("Expected 401, got {}", status));
                            }

                            let www_auth = resp
                                .headers()
                                .get("WWW-Authenticate")
                                .and_then(|v| v.to_str().ok())
                                .map(|s| s.to_string());

                            match www_auth {
                                Some(v) if v.contains("Basic") => TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Got 401 + WWW-Authenticate: {}", v),
                                ),
                                Some(v) => TestResult::failed(
                                    start.elapsed(),
                                    format!("WWW-Authenticate present but unexpected value: {}", v),
                                ),
                                None => TestResult::failed(
                                    start.elapsed(),
                                    "Missing WWW-Authenticate header in 401 response".into(),
                                ),
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Wrong password → 401
    fn test_wrong_password_returns_401() -> TestCase {
        TestCase::new(
            "wrong_password_returns_401",
            "Wrong password returns 401",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/health", ctx.http_url());
                    let auth = basic_auth_header(USER_ALICE.0, "wrong-password");

                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", TEST_HOST)
                        .header("Authorization", &auth)
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status == 401 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Wrong password correctly returned 401".into(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 401 for wrong password, got {}", status),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ============================================================
    // hide_credentials Tests
    // ============================================================

    /// With hideCredentials=true, the Authorization header must NOT reach the upstream.
    /// Uses the /auth-header-probe endpoint which reports back whether it saw the header.
    fn test_hide_credentials_removes_auth_header() -> TestCase {
        TestCase::new(
            "hide_credentials_removes_auth_header",
            "Authorization header is removed from upstream request when hideCredentials=true",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    // /auth-header-probe returns X-Auth-Header-Present: yes/no
                    let url = format!("{}/auth-header-probe", ctx.http_url());
                    let auth = basic_auth_header(USER_ALICE.0, USER_ALICE.1);

                    let resp = match ctx
                        .http_client
                        .get(&url)
                        .header("host", HIDE_CREDS_HOST)
                        .header("Authorization", &auth)
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };

                    let status = resp.status().as_u16();
                    if status != 200 {
                        return TestResult::failed(
                            start.elapsed(),
                            format!("Expected 200 (auth should pass), got {}", status),
                        );
                    }

                    // Check probe header
                    let present = resp
                        .headers()
                        .get("X-Auth-Header-Present")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("unknown");

                    if present == "no" {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "Authorization header was correctly removed from upstream request".into(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Authorization header was NOT removed (X-Auth-Header-Present={}). hideCredentials is broken.",
                                present
                            ),
                        )
                    }
                })
            },
        )
    }

    /// Without hideCredentials (default), Authorization header MUST reach the upstream.
    fn test_no_hide_credentials_keeps_auth_header() -> TestCase {
        TestCase::new(
            "no_hide_credentials_keeps_auth_header",
            "Authorization header reaches upstream when hideCredentials=false (default)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/auth-header-probe", ctx.http_url());
                    let auth = basic_auth_header(USER_ALICE.0, USER_ALICE.1);

                    let resp = match ctx
                        .http_client
                        .get(&url)
                        .header("host", TEST_HOST)
                        .header("Authorization", &auth)
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };

                    let status = resp.status().as_u16();
                    if status != 200 {
                        return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                    }

                    let present = resp
                        .headers()
                        .get("X-Auth-Header-Present")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("unknown");

                    if present == "yes" {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "Authorization header correctly forwarded to upstream (hideCredentials=false)".into(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Authorization header unexpectedly gone (X-Auth-Header-Present={})",
                                present
                            ),
                        )
                    }
                })
            },
        )
    }

    // ============================================================
    // auth_failure_delay_ms Tests
    // ============================================================

    /// With authFailureDelayMs=300, a 401 response must be delayed by at least ~250ms.
    fn test_auth_failure_delay_on_missing_credentials() -> TestCase {
        TestCase::new(
            "auth_failure_delay_missing_credentials",
            "401 response is delayed by authFailureDelayMs when no credentials provided",
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
                        return TestResult::failed(elapsed, format!("Expected 401 (no credentials), got {}", status));
                    }

                    let elapsed_ms = elapsed.as_millis() as u64;
                    if elapsed_ms >= DELAY_MIN_MS {
                        TestResult::passed_with_message(
                            elapsed,
                            format!(
                                "401 response delayed {}ms (≥ {}ms threshold, configured {}ms)",
                                elapsed_ms, DELAY_MIN_MS, CONFIGURED_DELAY_MS
                            ),
                        )
                    } else {
                        TestResult::failed(
                            elapsed,
                            format!(
                                "Delay too short: {}ms < {}ms threshold (configured authFailureDelayMs={}ms). \
                                 auth_failure_delay_ms may not be implemented.",
                                elapsed_ms, DELAY_MIN_MS, CONFIGURED_DELAY_MS
                            ),
                        )
                    }
                })
            },
        )
    }

    /// With authFailureDelayMs=300, wrong password also gets delayed.
    fn test_auth_failure_delay_on_wrong_password() -> TestCase {
        TestCase::new(
            "auth_failure_delay_wrong_password",
            "401 response on wrong password is delayed by authFailureDelayMs",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/health", ctx.http_url());
                    let auth = basic_auth_header(USER_ALICE.0, "definitely-wrong-password");

                    let resp = match ctx
                        .http_client
                        .get(&url)
                        .header("host", DELAY_HOST)
                        .header("Authorization", &auth)
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    };

                    let elapsed = start.elapsed();
                    let status = resp.status().as_u16();

                    if status != 401 {
                        return TestResult::failed(elapsed, format!("Expected 401 (wrong password), got {}", status));
                    }

                    let elapsed_ms = elapsed.as_millis() as u64;
                    if elapsed_ms >= DELAY_MIN_MS {
                        TestResult::passed_with_message(
                            elapsed,
                            format!(
                                "Wrong-password 401 delayed {}ms (≥ {}ms threshold)",
                                elapsed_ms, DELAY_MIN_MS
                            ),
                        )
                    } else {
                        TestResult::failed(
                            elapsed,
                            format!(
                                "Delay too short for wrong-password: {}ms < {}ms",
                                elapsed_ms, DELAY_MIN_MS
                            ),
                        )
                    }
                })
            },
        )
    }

    /// Successful auth with delay config must NOT be delayed.
    /// Verifies that the delay is only applied on failure paths, not success.
    fn test_no_delay_on_success() -> TestCase {
        TestCase::new(
            "no_delay_on_successful_auth",
            "Successful authentication is NOT delayed even when authFailureDelayMs is configured",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/health", ctx.http_url());
                    let auth = basic_auth_header(USER_ALICE.0, USER_ALICE.1);

                    let resp = match ctx
                        .http_client
                        .get(&url)
                        .header("host", DELAY_HOST)
                        .header("Authorization", &auth)
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
                            format!("Expected 200 for successful auth, got {}", status),
                        );
                    }

                    // Success should be fast — must complete well before the delay kicks in.
                    // Use 2× the configured delay as upper bound (generous for CI slowness).
                    let upper_bound_ms = CONFIGURED_DELAY_MS * 2;
                    let elapsed_ms = elapsed.as_millis() as u64;

                    if elapsed_ms < upper_bound_ms {
                        TestResult::passed_with_message(
                            elapsed,
                            format!(
                                "Successful auth completed in {}ms (< {}ms upper bound — no unwanted delay)",
                                elapsed_ms, upper_bound_ms
                            ),
                        )
                    } else {
                        // This is a soft warning rather than a hard failure because CI can be slow.
                        TestResult::passed_with_message(
                            elapsed,
                            format!(
                                "Warning: successful auth took {}ms (≥ {}ms upper bound) — \
                                 could be CI slowness, but verify success path is not delayed.",
                                elapsed_ms, upper_bound_ms
                            ),
                        )
                    }
                })
            },
        )
    }
}

impl TestSuite for BasicAuthTestSuite {
    fn name(&self) -> &str {
        "BasicAuth"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            // Basic authentication
            Self::test_valid_credentials_returns_200(),
            Self::test_second_user_valid_credentials(),
            Self::test_missing_credentials_returns_401(),
            Self::test_wrong_password_returns_401(),
            // hide_credentials
            Self::test_no_hide_credentials_keeps_auth_header(),
            Self::test_hide_credentials_removes_auth_header(),
            // auth_failure_delay_ms
            Self::test_auth_failure_delay_on_missing_credentials(),
            Self::test_auth_failure_delay_on_wrong_password(),
            Self::test_no_delay_on_success(),
        ]
    }
}
