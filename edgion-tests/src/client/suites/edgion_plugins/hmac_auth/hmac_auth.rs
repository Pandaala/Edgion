// HMAC Auth Plugin Test Suite
// - 04_Secret_default_hmac-credentials-grouped.yaml         # Two grouped Secrets
// - 05_EdgionPlugins_default_hmac-auth-grouped.yaml         # HmacAuth with partner-a + partner-b
// - 06_HTTPRoute_default_hmac-auth-grouped.yaml             # Route for grouped test

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::suites::edgion_plugins::hmac_test_utils::build_hmac_headers;
use std::time::Instant;

pub struct HmacAuthTestSuite;

const TEST_HOST: &str = "hmac-auth-test.example.com";
const ANON_HOST: &str = "hmac-auth-anonymous.example.com";
const VALID_USERNAME: &str = "alice";
const VALID_SECRET: &[u8] = b"alice-hmac-secret-key-must-be-at-least-32-bytes-long";
const BAD_SECRET: &[u8] = b"alice-hmac-secret-key-invalid-for-signature-tests";
const GROUPED_HOST: &str = "hmac-auth-grouped.example.com";
const GROUPED_USER_PARTNER_A: &str = "partner-a-user";
const GROUPED_SECRET_PARTNER_A: &[u8] = b"partner-a-secret-key-at-least-32-bytes-long-padding";
const GROUPED_USER_PARTNER_B: &str = "partner-b-user";
const GROUPED_SECRET_PARTNER_B: &[u8] = b"partner-b-secret-key-at-least-32-bytes-long-padding";

impl HmacAuthTestSuite {
    fn test_valid_signature_returns_200() -> TestCase {
        TestCase::new(
            "valid_signature_returns_200",
            "Valid HMAC signature returns 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let signed =
                        build_hmac_headers(VALID_USERNAME, VALID_SECRET, "GET", "/health", None, TEST_HOST, None);
                    let url = format!("{}/health", ctx.http_url());

                    let request = ctx
                        .http_client
                        .get(&url)
                        .header("host", TEST_HOST)
                        .header("date", signed.date)
                        .header("authorization", signed.authorization);

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Valid HMAC signature accepted".to_string(),
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

    fn test_missing_auth_returns_401() -> TestCase {
        TestCase::new(
            "missing_auth_returns_401",
            "Missing Authorization header returns 401",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/health", ctx.http_url());

                    let request = ctx.http_client.get(&url).header("host", TEST_HOST);

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 401 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Missing Authorization rejected with 401".to_string(),
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

    fn test_invalid_signature_returns_401() -> TestCase {
        TestCase::new(
            "invalid_signature_returns_401",
            "Invalid HMAC signature returns 401",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let signed =
                        build_hmac_headers(VALID_USERNAME, BAD_SECRET, "GET", "/health", None, TEST_HOST, None);
                    let url = format!("{}/health", ctx.http_url());

                    let request = ctx
                        .http_client
                        .get(&url)
                        .header("host", TEST_HOST)
                        .header("date", signed.date)
                        .header("authorization", signed.authorization);

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 401 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Invalid signature rejected with 401".to_string(),
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

    fn test_upstream_headers_forwarded() -> TestCase {
        TestCase::new(
            "upstream_headers_forwarded",
            "Credential metadata headers are forwarded upstream",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let signed =
                        build_hmac_headers(VALID_USERNAME, VALID_SECRET, "GET", "/headers", None, TEST_HOST, None);
                    let url = format!("{}/headers", ctx.http_url());

                    let request = ctx
                        .http_client
                        .get(&url)
                        .header("host", TEST_HOST)
                        .header("date", signed.date)
                        .header("authorization", signed.authorization);

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                            }

                            let body = response.text().await.unwrap_or_default().to_lowercase();
                            let has_user = body.contains("x-consumer-username") && body.contains("alice");
                            let has_customer = body.contains("x-customer-id") && body.contains("cust-001");
                            let has_filtered = body.contains("x-team");

                            if has_user && has_customer && !has_filtered {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Upstream metadata headers forwarded with whitelist filtering".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Header forwarding mismatch: user={}, customer={}, filtered={}",
                                        has_user, has_customer, has_filtered
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

    fn test_hide_credentials_removes_auth_header() -> TestCase {
        TestCase::new(
            "hide_credentials_removes_auth_header",
            "hideCredentials removes Authorization header before upstream",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let signed =
                        build_hmac_headers(VALID_USERNAME, VALID_SECRET, "GET", "/headers", None, TEST_HOST, None);
                    let url = format!("{}/headers", ctx.http_url());

                    let request = ctx
                        .http_client
                        .get(&url)
                        .header("host", TEST_HOST)
                        .header("date", signed.date)
                        .header("authorization", signed.authorization);

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                            }

                            let body = response.text().await.unwrap_or_default().to_lowercase();
                            if body.contains("authorization") {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Authorization header still exists in upstream body: {}", body),
                                )
                            } else {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Authorization header removed before upstream".to_string(),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_anonymous_access_without_auth() -> TestCase {
        TestCase::new(
            "anonymous_access_without_auth",
            "Anonymous config allows requests without Authorization",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/headers", ctx.http_url());

                    let request = ctx.http_client.get(&url).header("host", ANON_HOST);

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                            }

                            let body = response.text().await.unwrap_or_default().to_lowercase();
                            let has_anon = body.contains("x-anonymous-consumer") && body.contains("true");
                            let has_user = body.contains("x-consumer-username") && body.contains("anonymous-user");

                            if has_anon && has_user {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Anonymous request accepted with anonymous upstream headers".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Anonymous headers missing. body={} ", body),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_grouped_partner_a_returns_200() -> TestCase {
        TestCase::new(
            "grouped_partner_a_returns_200",
            "Signed request for partner-a user returns 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/health", ctx.http_url());
                    let signed = build_hmac_headers(
                        GROUPED_USER_PARTNER_A,
                        GROUPED_SECRET_PARTNER_A,
                        "GET",
                        "/health",
                        None,
                        GROUPED_HOST,
                        None,
                    );

                    let result = client
                        .get(&url)
                        .header("host", GROUPED_HOST)
                        .header("date", signed.date)
                        .header("Authorization", signed.authorization)
                        .send()
                        .await;

                    match result {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "partner-a signed request accepted".to_string(),
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

    fn test_grouped_partner_b_returns_200() -> TestCase {
        TestCase::new(
            "grouped_partner_b_returns_200",
            "Signed request for partner-b user returns 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/health", ctx.http_url());
                    let signed = build_hmac_headers(
                        GROUPED_USER_PARTNER_B,
                        GROUPED_SECRET_PARTNER_B,
                        "GET",
                        "/health",
                        None,
                        GROUPED_HOST,
                        None,
                    );

                    let result = client
                        .get(&url)
                        .header("host", GROUPED_HOST)
                        .header("date", signed.date)
                        .header("Authorization", signed.authorization)
                        .send()
                        .await;

                    match result {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "partner-b signed request accepted".to_string(),
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
}

impl TestSuite for HmacAuthTestSuite {
    fn name(&self) -> &str {
        "HmacAuth"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_valid_signature_returns_200(),
            Self::test_missing_auth_returns_401(),
            Self::test_invalid_signature_returns_401(),
            Self::test_upstream_headers_forwarded(),
            Self::test_hide_credentials_removes_auth_header(),
            Self::test_anonymous_access_without_auth(),
            Self::test_grouped_partner_a_returns_200(),
            Self::test_grouped_partner_b_returns_200(),
        ]
    }
}
