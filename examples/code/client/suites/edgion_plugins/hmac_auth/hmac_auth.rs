// HMAC Auth Plugin Test Suite

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use chrono::Utc;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::time::Instant;

type HmacSha256 = Hmac<Sha256>;

pub struct HmacAuthTestSuite;

const TEST_HOST: &str = "hmac-auth-test.example.com";
const ANON_HOST: &str = "hmac-auth-anonymous.example.com";
const VALID_USERNAME: &str = "alice";
const VALID_SECRET: &[u8] = b"alice-hmac-secret-key-must-be-at-least-32-bytes-long";
const BAD_SECRET: &[u8] = b"alice-hmac-secret-key-invalid-for-signature-tests";

fn build_signing_string(method: &str, path: &str, query: Option<&str>, host: &str, date: &str) -> String {
    let mut request_target = path.to_string();
    if let Some(q) = query {
        if !q.is_empty() {
            request_target.push('?');
            request_target.push_str(q);
        }
    }

    format!(
        "@request-target: {} {}\nhost: {}\ndate: {}",
        method.to_ascii_lowercase(),
        request_target,
        host,
        date
    )
}

fn build_signature(secret: &[u8], signing_string: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(secret).expect("valid HMAC key");
    mac.update(signing_string.as_bytes());
    STANDARD.encode(mac.finalize().into_bytes())
}

fn build_authorization(username: &str, secret: &[u8], method: &str, path: &str, host: &str, date: &str) -> String {
    let signing = build_signing_string(method, path, None, host, date);
    let signature = build_signature(secret, &signing);

    format!(
        r#"hmac username="{}", algorithm="hmac-sha256", headers="@request-target host date", signature="{}""#,
        username, signature
    )
}

impl HmacAuthTestSuite {
    fn test_valid_signature_returns_200() -> TestCase {
        TestCase::new(
            "valid_signature_returns_200",
            "Valid HMAC signature returns 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let date = Utc::now().to_rfc2822();
                    let auth = build_authorization(VALID_USERNAME, VALID_SECRET, "GET", "/health", TEST_HOST, &date);
                    let url = format!("{}/health", ctx.http_url());

                    let request = ctx
                        .http_client
                        .get(&url)
                        .header("host", TEST_HOST)
                        .header("date", date)
                        .header("authorization", auth);

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
                    let date = Utc::now().to_rfc2822();
                    let auth = build_authorization(VALID_USERNAME, BAD_SECRET, "GET", "/health", TEST_HOST, &date);
                    let url = format!("{}/health", ctx.http_url());

                    let request = ctx
                        .http_client
                        .get(&url)
                        .header("host", TEST_HOST)
                        .header("date", date)
                        .header("authorization", auth);

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
                    let date = Utc::now().to_rfc2822();
                    let auth = build_authorization(VALID_USERNAME, VALID_SECRET, "GET", "/headers", TEST_HOST, &date);
                    let url = format!("{}/headers", ctx.http_url());

                    let request = ctx
                        .http_client
                        .get(&url)
                        .header("host", TEST_HOST)
                        .header("date", date)
                        .header("authorization", auth);

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
                    let date = Utc::now().to_rfc2822();
                    let auth = build_authorization(VALID_USERNAME, VALID_SECRET, "GET", "/headers", TEST_HOST, &date);
                    let url = format!("{}/headers", ctx.http_url());

                    let request = ctx
                        .http_client
                        .get(&url)
                        .header("host", TEST_HOST)
                        .header("date", date)
                        .header("authorization", auth);

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
        ]
    }
}
