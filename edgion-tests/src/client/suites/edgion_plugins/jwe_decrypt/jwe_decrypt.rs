// JWE Decrypt Plugin Test Suite
//
// Required config files (in examples/test/conf/EdgionPlugins/JweDecrypt/):
// - 01_Secret_default_jwe-secret.yaml            # 32-byte symmetric key
// - EdgionPlugins_default_jwe-decrypt.yaml       # JweDecrypt plugin config
// - HTTPRoute_default_jwe-decrypt-test.yaml      # Route with host: jwe-decrypt-test.example.com

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use aes_gcm::aead::{AeadInPlace, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde_json::json;
use std::time::Instant;

pub struct JweDecryptTestSuite;

const TEST_HOST: &str = "jwe-decrypt-test.example.com";
const JWE_KEY: &[u8] = b"0123456789abcdef0123456789abcdef";

fn build_compact_jwe(payload: &str, enc: &str) -> String {
    let protected = json!({
        "alg": "dir",
        "enc": enc
    })
    .to_string();
    let protected_segment = URL_SAFE_NO_PAD.encode(protected.as_bytes());

    let iv = b"fixed-12-byt";
    let mut ciphertext = payload.as_bytes().to_vec();
    let cipher = Aes256Gcm::new_from_slice(JWE_KEY).expect("invalid test key");
    let nonce = Nonce::from_slice(iv);
    let tag = cipher
        .encrypt_in_place_detached(nonce, protected_segment.as_bytes(), &mut ciphertext)
        .expect("failed to encrypt test payload");

    format!(
        "{}..{}.{}.{}",
        protected_segment,
        URL_SAFE_NO_PAD.encode(iv),
        URL_SAFE_NO_PAD.encode(ciphertext),
        URL_SAFE_NO_PAD.encode(tag)
    )
}

impl JweDecryptTestSuite {
    fn test_valid_jwe_returns_200() -> TestCase {
        TestCase::new(
            "valid_jwe_returns_200",
            "Valid compact JWE in authorization header returns 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let payload = r#"{"uid":"100","user":{"department":"eng"},"permissions":{"admin":true}}"#;
                    let token = build_compact_jwe(payload, "A256GCM");
                    let client = &ctx.http_client;
                    let url = format!("{}/health", ctx.http_url());

                    let request = client
                        .get(&url)
                        .header("host", TEST_HOST)
                        .header("authorization", format!("Bearer {}", token));

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Valid JWE accepted, returned 200".to_string(),
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

    fn test_missing_token_returns_403() -> TestCase {
        TestCase::new(
            "missing_token_returns_403",
            "Missing JWE token returns 403 in strict mode",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/health", ctx.http_url());

                    let request = client.get(&url).header("host", TEST_HOST);
                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 403 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Missing token returns 403 as expected".to_string(),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected 403, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_invalid_jwe_format_returns_400() -> TestCase {
        TestCase::new(
            "invalid_jwe_format_returns_400",
            "Invalid JWE format returns 400",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/health", ctx.http_url());

                    let request = client
                        .get(&url)
                        .header("host", TEST_HOST)
                        .header("authorization", "Bearer invalid.jwe.token");
                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 400 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Invalid JWE format returns 400 as expected".to_string(),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected 400, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_unsupported_algorithm_returns_400() -> TestCase {
        TestCase::new(
            "unsupported_algorithm_returns_400",
            "Unsupported enc algorithm in JWE header returns 400",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let token = build_compact_jwe(r#"{"uid":"100"}"#, "A128GCM");
                    let client = &ctx.http_client;
                    let url = format!("{}/health", ctx.http_url());

                    let request = client
                        .get(&url)
                        .header("host", TEST_HOST)
                        .header("authorization", format!("Bearer {}", token));
                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 400 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Unsupported algorithm returns 400 as expected".to_string(),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected 400, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_payload_headers_mapped() -> TestCase {
        TestCase::new(
            "payload_headers_mapped",
            "payloadToHeaders mapping injects expected upstream headers",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let payload = r#"{"uid":"100","user":{"department":"eng"},"permissions":{"admin":true}}"#;
                    let token = build_compact_jwe(payload, "A256GCM");
                    let client = &ctx.http_client;
                    let url = format!("{}/headers", ctx.http_url());

                    let request = client
                        .get(&url)
                        .header("host", TEST_HOST)
                        .header("authorization", format!("Bearer {}", token));

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                            }

                            let body = response.text().await.unwrap_or_default().to_lowercase();
                            let has_uid = body.contains("x-user-id") && body.contains("100");
                            let has_dept = body.contains("x-user-dept") && body.contains("eng");
                            let has_admin = body.contains("x-is-admin") && body.contains("true");

                            if has_uid && has_dept && has_admin {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Mapped headers found: X-User-ID, X-User-Dept, X-Is-Admin".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Mapped headers missing. uid={}, dept={}, admin={}, body={}",
                                        has_uid, has_dept, has_admin, body
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
}

impl TestSuite for JweDecryptTestSuite {
    fn name(&self) -> &str {
        "JweDecrypt"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_valid_jwe_returns_200(),
            Self::test_missing_token_returns_403(),
            Self::test_invalid_jwe_format_returns_400(),
            Self::test_unsupported_algorithm_returns_400(),
            Self::test_payload_headers_mapped(),
        ]
    }
}
