// JWT Auth Plugin Test Suite
//
// Required config files (in edgion-tests/conf/EdgionPlugins/JwtAuth/):
// - 01_Secret_default_jwt-secret.yaml            # JWT secret: "my-jwt-secret-key-32-chars-long!!"
// - EdgionPlugins_default_jwt-auth.yaml          # JwtAuth plugin with HS256 (issuerGroups: default)
// - HTTPRoute_default_jwt-auth-test.yaml         # Route with host: jwt-test.example.com
// - 02_Secret_default_jwt-credentials-grouped.yaml      # Two grouped Secrets
// - 03_EdgionPlugins_default_jwt-auth-grouped.yaml      # JwtAuth with partner-a + partner-b
// - 04_HTTPRoute_default_jwt-auth-grouped.yaml          # Route for grouped test
//
// Also requires base config (in edgion-tests/conf/EdgionPlugins/base/):
// - Gateway.yaml                              # Gateway for EdgionPlugins tests

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde_json::json;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub struct JwtAuthTestSuite;

/// JWT secret used in default-group tests (must match 01_Secret_default_jwt-secret.yaml decoded)
const JWT_SECRET: &[u8] = b"my-jwt-secret-key-32-chars-long!!";

/// Default issuer — must match the `issuers` list in EdgionPlugins_default_jwt-auth.yaml
const DEFAULT_ISS: &str = "https://test.example";

/// Default kid — must match the `key:` field (base64 of "default-kid") in 01_Secret_default_jwt-secret.yaml
const DEFAULT_KID: &str = "default-kid";

/// Grouped test constants
const GROUPED_HOST: &str = "jwt-auth-grouped.example.com";
const GROUPED_ISS_A: &str = "https://a.example";
const GROUPED_ISS_B: &str = "https://b.example";
const GROUPED_KID_A: &str = "ka";
const GROUPED_KID_B: &str = "kb";
const GROUPED_SECRET_A: &[u8] = b"partner-a-jwt-secret-32-chars-long-pad";
const GROUPED_SECRET_B: &[u8] = b"partner-b-jwt-secret-32-chars-long-pad";

/// Build a HS256 JWT with iss, kid (key claim), sub and exp.
/// Uses the `jsonwebtoken` crate (v9) — the same library used by the gateway.
fn build_jwt_hs256(iss: &str, kid: &str, secret: &[u8], sub: &str) -> String {
    let exp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() + 3600;
    let claims = json!({
        "iss": iss,
        "key": kid,
        "sub": sub,
        "exp": exp,
    });
    let header = Header::new(Algorithm::HS256);
    encode(&header, &claims, &EncodingKey::from_secret(secret)).unwrap()
}

/// Build an expired HS256 JWT (exp in the past).
fn build_expired_jwt_hs256(iss: &str, kid: &str, secret: &[u8], sub: &str) -> String {
    let exp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .saturating_sub(3600);
    let claims = json!({
        "iss": iss,
        "key": kid,
        "sub": sub,
        "exp": exp,
    });
    let header = Header::new(Algorithm::HS256);
    encode(&header, &claims, &EncodingKey::from_secret(secret)).unwrap()
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

                    let jwt = build_jwt_hs256(DEFAULT_ISS, DEFAULT_KID, JWT_SECRET, "test-user");

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
                            if status != 401 {
                                return TestResult::failed(start.elapsed(), format!("Expected 401, got {}", status));
                            }
                            // Token present but invalid -> RFC 6750 §3.1: challenge SHOULD carry error="invalid_token".
                            let www_auth = response
                                .headers()
                                .get("WWW-Authenticate")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("");
                            if www_auth.contains(r#"error="invalid_token""#) {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("401 with WWW-Authenticate: {}", www_auth),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected error=\"invalid_token\" in WWW-Authenticate, got: {}",
                                        www_auth
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

                    let jwt = build_expired_jwt_hs256(DEFAULT_ISS, DEFAULT_KID, JWT_SECRET, "test-user");

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

    /// Test: JWT in query parameter works only when the route opts in.
    /// The backing config sets `allowTokenInQuery: true`; the query path is off
    /// by default (RFC 6750 §2.3/§5.3).
    fn test_jwt_in_query() -> TestCase {
        TestCase::new(
            "jwt_in_query",
            "Valid JWT in query parameter returns 200 (allowTokenInQuery enabled)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;

                    let jwt = build_jwt_hs256(DEFAULT_ISS, DEFAULT_KID, JWT_SECRET, "test-user");
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

                    let jwt = build_jwt_hs256(DEFAULT_ISS, DEFAULT_KID, JWT_SECRET, "test-user");

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
                                // No credentials presented -> RFC 6750 §3.1 says the challenge SHOULD NOT
                                // carry an error code; only the bare `Bearer realm="..."` is expected.
                                Some(value) if value.starts_with("Bearer") && !value.contains("error=") => {
                                    TestResult::passed_with_message(
                                        start.elapsed(),
                                        format!("Got WWW-Authenticate: {}", value),
                                    )
                                }
                                Some(value) => TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "WWW-Authenticate must omit error= for a no-credentials request, got: {}",
                                        value
                                    ),
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
            "JWT with sub claim returns 200 (sub used for username extraction in OK log)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/health", ctx.http_url());

                    // Token includes iss + key (required) and sub (for OK log username extraction)
                    let jwt = build_jwt_hs256(DEFAULT_ISS, DEFAULT_KID, JWT_SECRET, "user@example.com");

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
                    let valid_jwt = build_jwt_hs256(DEFAULT_ISS, DEFAULT_KID, JWT_SECRET, "valid-user");
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

    fn test_stage_logs_contain_jwt_auth() -> TestCase {
        TestCase::new(
            "stage_logs_contain_jwt_auth",
            "Access log stage_logs contains JwtAuth plugin entry on successful auth",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let trace_id = format!("test-jwt-auth-{}", uuid::Uuid::new_v4());
                    let url = format!("{}/health", ctx.http_url());
                    let jwt = build_jwt_hs256(DEFAULT_ISS, DEFAULT_KID, JWT_SECRET, "test-user");

                    let _ = ctx
                        .http_client
                        .get(&url)
                        .header("host", "jwt-test.example.com")
                        .header("Authorization", format!("Bearer {}", jwt))
                        .header("x-trace-id", &trace_id)
                        .header("access_log", "test_store")
                        .send()
                        .await;

                    let entry = match ctx
                        .access_log_client()
                        .get_access_log_with_retry(&trace_id, 10, 200)
                        .await
                    {
                        Ok(e) => e,
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Access log not found: {}", e)),
                    };

                    let found = entry.data["stage_logs"]
                        .as_array()
                        .map(|stages| {
                            stages.iter().any(|stage| {
                                stage["edgion_plugins"]
                                    .as_array()
                                    .map(|eps| {
                                        eps.iter().any(|ep| {
                                            ep["logs"]
                                                .as_array()
                                                .map(|logs| logs.iter().any(|l| l["name"].as_str() == Some("JwtAuth")))
                                                .unwrap_or(false)
                                        })
                                    })
                                    .unwrap_or(false)
                            })
                        })
                        .unwrap_or(false);

                    if found {
                        TestResult::passed_with_message(start.elapsed(), "JwtAuth found in stage_logs".to_string())
                    } else {
                        TestResult::failed(start.elapsed(), "JwtAuth not found in stage_logs".to_string())
                    }
                })
            },
        )
    }

    /// Test: JWT from partner-a group is routed and verified correctly
    fn test_grouped_partner_a_returns_200() -> TestCase {
        TestCase::new(
            "grouped_partner_a_returns_200",
            "JWT from partner-a routed and verified",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let token = build_jwt_hs256(GROUPED_ISS_A, GROUPED_KID_A, GROUPED_SECRET_A, "alice");
                    let url = format!("{}/health", ctx.http_url());
                    let result = ctx
                        .http_client
                        .get(&url)
                        .header("host", GROUPED_HOST)
                        .header("authorization", format!("Bearer {}", token))
                        .send()
                        .await;
                    match result {
                        Ok(r) if r.status().as_u16() == 200 => {
                            TestResult::passed_with_message(start.elapsed(), "partner-a 200".into())
                        }
                        Ok(r) => TestResult::failed(start.elapsed(), format!("expected 200, got {}", r.status())),
                        Err(e) => TestResult::failed(start.elapsed(), format!("{}", e)),
                    }
                })
            },
        )
    }

    /// Test: JWT from partner-b group is routed and verified correctly
    fn test_grouped_partner_b_returns_200() -> TestCase {
        TestCase::new(
            "grouped_partner_b_returns_200",
            "JWT from partner-b routed and verified",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let token = build_jwt_hs256(GROUPED_ISS_B, GROUPED_KID_B, GROUPED_SECRET_B, "bob");
                    let url = format!("{}/health", ctx.http_url());
                    let result = ctx
                        .http_client
                        .get(&url)
                        .header("host", GROUPED_HOST)
                        .header("authorization", format!("Bearer {}", token))
                        .send()
                        .await;
                    match result {
                        Ok(r) if r.status().as_u16() == 200 => {
                            TestResult::passed_with_message(start.elapsed(), "partner-b 200".into())
                        }
                        Ok(r) => TestResult::failed(start.elapsed(), format!("expected 200, got {}", r.status())),
                        Err(e) => TestResult::failed(start.elapsed(), format!("{}", e)),
                    }
                })
            },
        )
    }

    /// Test: Token with unknown iss returns 401 (mismatched issuer not in any group)
    fn test_grouped_mismatched_iss_returns_401() -> TestCase {
        TestCase::new(
            "grouped_mismatched_iss_returns_401",
            "Token with unknown iss returns 401",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    // Sign with partner-a's secret but claim an iss not in any group
                    let token = build_jwt_hs256("https://unknown.example", GROUPED_KID_A, GROUPED_SECRET_A, "alice");
                    let url = format!("{}/health", ctx.http_url());
                    let result = ctx
                        .http_client
                        .get(&url)
                        .header("host", GROUPED_HOST)
                        .header("authorization", format!("Bearer {}", token))
                        .send()
                        .await;
                    match result {
                        Ok(r) if r.status().as_u16() == 401 => {
                            TestResult::passed_with_message(start.elapsed(), "unknown iss rejected with 401".into())
                        }
                        Ok(r) => TestResult::failed(
                            start.elapsed(),
                            format!("expected 401 for unknown iss, got {}", r.status()),
                        ),
                        Err(e) => TestResult::failed(start.elapsed(), format!("{}", e)),
                    }
                })
            },
        )
    }

    /// Test: Token with trailing-slash iss returns 401 (no normalization — spec §11)
    fn test_grouped_iss_trailing_slash_returns_401() -> TestCase {
        TestCase::new(
            "grouped_iss_trailing_slash_returns_401",
            "Token with trailing-slash iss returns 401 (no iss normalization)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    // Group iss is "https://a.example" (no slash); token adds trailing slash
                    let token = build_jwt_hs256("https://a.example/", GROUPED_KID_A, GROUPED_SECRET_A, "alice");
                    let url = format!("{}/health", ctx.http_url());
                    let result = ctx
                        .http_client
                        .get(&url)
                        .header("host", GROUPED_HOST)
                        .header("authorization", format!("Bearer {}", token))
                        .send()
                        .await;
                    match result {
                        Ok(r) if r.status().as_u16() == 401 => TestResult::passed_with_message(
                            start.elapsed(),
                            "trailing-slash iss rejected with 401 (no normalization)".into(),
                        ),
                        Ok(r) => TestResult::failed(
                            start.elapsed(),
                            format!("expected 401 for trailing-slash iss, got {}", r.status()),
                        ),
                        Err(e) => TestResult::failed(start.elapsed(), format!("{}", e)),
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
            // P2: Username extraction (sub claim)
            Self::test_jwt_with_sub_claim(),
            // access log regression
            Self::test_stage_logs_contain_jwt_auth(),
            // Grouped issuer tests
            Self::test_grouped_partner_a_returns_200(),
            Self::test_grouped_partner_b_returns_200(),
            Self::test_grouped_mismatched_iss_returns_401(),
            Self::test_grouped_iss_trailing_slash_returns_401(),
        ]
    }
}
