// JWT Auth Plugin Test Suite
//
// Required config files (in examples/test/conf/EdgionPlugins/JwtAuth/):
// - Secret_default_jwt-secret.yaml            # JWT secret: "my-jwt-secret-key-32-chars-long!!"
// - EdgionPlugins_default_jwt-auth.yaml       # JwtAuth plugin with HS256
// - HTTPRoute_default_jwt-auth-test.yaml      # Route with host: jwt-test.example.com
//
// Also requires base config (in examples/test/conf/EdgionPlugins/base/):
// - Gateway.yaml                              # Gateway for EdgionPlugins tests

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub struct JwtAuthTestSuite;

/// JWT secret used in test (must match Secret_default_jwt-secret.yaml)
const JWT_SECRET: &str = "my-jwt-secret-key-32-chars-long!!";

type HmacSha256 = Hmac<Sha256>;

/// Generate a HS256 JWT token with custom claims
fn generate_jwt_with_claims(claims_json: &str) -> String {
    // Header: {"alg":"HS256","typ":"JWT"}
    let header = r#"{"alg":"HS256","typ":"JWT"}"#;
    let header_b64 = URL_SAFE_NO_PAD.encode(header);
    let payload_b64 = URL_SAFE_NO_PAD.encode(claims_json);

    // Signature: HMAC-SHA256
    let message = format!("{}.{}", header_b64, payload_b64);
    let mut mac = HmacSha256::new_from_slice(JWT_SECRET.as_bytes()).unwrap();
    mac.update(message.as_bytes());
    let signature = mac.finalize().into_bytes();
    let signature_b64 = URL_SAFE_NO_PAD.encode(&signature);

    format!("{}.{}.{}", header_b64, payload_b64, signature_b64)
}

/// Generate a simple HS256 JWT token
fn generate_jwt(key_claim: &str, exp_offset_secs: i64) -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
    let exp = now + exp_offset_secs;
    let payload = format!(r#"{{"key":"{}","exp":{}}}"#, key_claim, exp);
    generate_jwt_with_claims(&payload)
}

/// Generate JWT with sub claim (for username extraction tests)
fn generate_jwt_with_sub(sub: &str, exp_offset_secs: i64) -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
    let exp = now + exp_offset_secs;
    let payload = format!(r#"{{"sub":"{}","exp":{}}}"#, sub, exp);
    generate_jwt_with_claims(&payload)
}

impl JwtAuthTestSuite {
    /// Test: Valid JWT in Authorization header should return 200
    fn test_valid_jwt_header() -> TestCase {
        TestCase::new(
            "valid_jwt_header",
            "Valid JWT in Authorization header returns 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/health", ctx.http_url());

                    // Generate valid JWT (expires in 1 hour)
                    let jwt = generate_jwt("test-user", 3600);

                    let request = client
                        .get(&url)
                        .header("host", "jwt-test.example.com")
                        .header("Authorization", format!("Bearer {}", jwt));

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Valid JWT accepted, returned 200".to_string(),
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

    /// Test: No token should return 401
    fn test_no_token_returns_401() -> TestCase {
        TestCase::new(
            "no_token_returns_401",
            "Request without JWT returns 401",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/health", ctx.http_url());

                    let request = client.get(&url).header("host", "jwt-test.example.com");

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 401 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "No token returns 401 as expected".to_string(),
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

    /// Test: Invalid token should return 401
    fn test_invalid_token_returns_401() -> TestCase {
        TestCase::new(
            "invalid_token_returns_401",
            "Invalid JWT returns 401",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/health", ctx.http_url());

                    let request = client
                        .get(&url)
                        .header("host", "jwt-test.example.com")
                        .header("Authorization", "Bearer invalid.token.here");

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 401 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Invalid token returns 401 as expected".to_string(),
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

    /// Test: Expired token should return 401
    fn test_expired_token_returns_401() -> TestCase {
        TestCase::new(
            "expired_token_returns_401",
            "Expired JWT returns 401",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/health", ctx.http_url());

                    // Generate expired JWT (expired 1 hour ago)
                    let jwt = generate_jwt("test-user", -3600);

                    let request = client
                        .get(&url)
                        .header("host", "jwt-test.example.com")
                        .header("Authorization", format!("Bearer {}", jwt));

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 401 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Expired token returns 401 as expected".to_string(),
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

    /// Test: JWT in query parameter should work
    fn test_jwt_in_query() -> TestCase {
        TestCase::new(
            "jwt_in_query",
            "Valid JWT in query parameter returns 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;

                    // Generate valid JWT
                    let jwt = generate_jwt("test-user", 3600);
                    let url = format!("{}/health?jwt={}", ctx.http_url(), jwt);

                    let request = client.get(&url).header("host", "jwt-test.example.com");

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "JWT in query accepted, returned 200".to_string(),
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

    /// Test: JWT in cookie should work
    fn test_jwt_in_cookie() -> TestCase {
        TestCase::new(
            "jwt_in_cookie",
            "Valid JWT in cookie returns 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/health", ctx.http_url());

                    // Generate valid JWT
                    let jwt = generate_jwt("test-user", 3600);

                    let request = client
                        .get(&url)
                        .header("host", "jwt-test.example.com")
                        .header("Cookie", format!("jwt={}", jwt));

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "JWT in cookie accepted, returned 200".to_string(),
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
            "401 response includes WWW-Authenticate: Bearer header",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/health", ctx.http_url());

                    let request = client.get(&url).header("host", "jwt-test.example.com");

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status != 401 {
                                return TestResult::failed(start.elapsed(), format!("Expected 401, got {}", status));
                            }

                            // Check for WWW-Authenticate header
                            let www_auth = response.headers().get("WWW-Authenticate").and_then(|v| v.to_str().ok());

                            match www_auth {
                                Some(value) if value.starts_with("Bearer") => TestResult::passed_with_message(
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

    /// Test: JWT with sub claim should be accepted (username extraction)
    fn test_jwt_with_sub_claim() -> TestCase {
        TestCase::new(
            "jwt_with_sub_claim",
            "JWT with sub claim (no key claim) returns 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/health", ctx.http_url());

                    // Generate JWT with sub instead of key
                    let jwt = generate_jwt_with_sub("user@example.com", 3600);

                    let request = client
                        .get(&url)
                        .header("host", "jwt-test.example.com")
                        .header("Authorization", format!("Bearer {}", jwt));

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "JWT with sub claim accepted".to_string(),
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

    /// Test: Token extraction priority (Header > Query > Cookie)
    fn test_token_extraction_priority() -> TestCase {
        TestCase::new(
            "token_extraction_priority",
            "Header token takes priority over query/cookie",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;

                    // Valid token in header
                    let valid_jwt = generate_jwt("valid-user", 3600);
                    // Invalid token in query
                    let url = format!("{}/health?jwt=invalid.token.here", ctx.http_url());

                    let request = client
                        .get(&url)
                        .header("host", "jwt-test.example.com")
                        .header("Authorization", format!("Bearer {}", valid_jwt))
                        .header("Cookie", "jwt=also.invalid.token");

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Header token took priority, request succeeded".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200 (header token should be used), got {}", status),
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

impl TestSuite for JwtAuthTestSuite {
    fn name(&self) -> &str {
        "JwtAuth"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            // Basic tests
            Self::test_valid_jwt_header(),
            Self::test_no_token_returns_401(),
            Self::test_invalid_token_returns_401(),
            Self::test_expired_token_returns_401(),
            // Token extraction tests
            Self::test_jwt_in_query(),
            Self::test_jwt_in_cookie(),
            Self::test_token_extraction_priority(),
            // P0: WWW-Authenticate header
            Self::test_www_authenticate_header(),
            // P2: Username extraction (sub claim fallback)
            Self::test_jwt_with_sub_claim(),
        ]
    }
}
