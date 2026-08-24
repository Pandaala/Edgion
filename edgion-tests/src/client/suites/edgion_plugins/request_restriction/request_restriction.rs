// RequestRestriction Plugin Test Suite
//
// Covers:
// - Header deny by value (User-Agent Bot blocking)
// - Path deny (admin path blocking)
// - Method allow (GET-only API)
// - Header required (must carry Token)
// - Combined rules

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::suites::edgion_plugins::hmac_test_utils::build_hmac_headers;
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde_json::json;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub struct RequestRestrictionTestSuite;

/// JWT signing material — must match 08_Secret_default_rr-jwt-secret.yaml decoded.
const RR_JWT_SECRET: &[u8] = b"my-jwt-secret-key-32-chars-long!!";
/// Issuer — must match the `issuers` list in 09_EdgionPlugins_jwt-claim.yaml.
const RR_JWT_ISS: &str = "https://rr.example";
/// Key id (kid) — must match the `key:` field (base64 of "rr-kid") in the secret.
const RR_JWT_KID: &str = "rr-kid";
/// Host carrying the JwtClaim chain route (shared request-restriction route host).
const RR_HOST: &str = "request-restriction.example.com";
const RR_HMAC_USERNAME: &str = "rr-alice";
const RR_HMAC_SECRET: &[u8] = b"rr-hmac-secret-key-must-be-at-least-32-bytes";
const RR_HMAC_BAD_SECRET: &[u8] = b"rr-hmac-wrong-secret-key-must-also-be-long";

/// Build an HS256 JWT carrying a nested `realm_access.roles` array claim.
fn build_jwt_with_roles(roles: &[&str]) -> String {
    let exp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() + 3600;
    let claims = json!({
        "iss": RR_JWT_ISS,
        "key": RR_JWT_KID,
        "sub": "test-user",
        "exp": exp,
        "realm_access": { "roles": roles },
    });
    encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(RR_JWT_SECRET),
    )
    .unwrap()
}

/// Build an HS256 JWT WITHOUT the realm_access claim (to exercise the missing-claim branch).
fn build_jwt_without_roles() -> String {
    let exp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() + 3600;
    let claims = json!({
        "iss": RR_JWT_ISS,
        "key": RR_JWT_KID,
        "sub": "test-user",
        "exp": exp,
    });
    encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(RR_JWT_SECRET),
    )
    .unwrap()
}

impl RequestRestrictionTestSuite {
    // ==================== 1. Header deny by value ====================
    fn test_header_deny_blocks_bot() -> TestCase {
        TestCase::new(
            "header_deny_blocks_bot",
            "Header : Bot User-Agent  (403)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/test/header-deny/api", ctx.http_url());

                    let request = client
                        .get(&url)
                        .header("host", "request-restriction.example.com")
                        .header("User-Agent", "Googlebot/2.1");

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 403 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Bot blocked with status {}", status),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected status 403, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_header_deny_allows_normal() -> TestCase {
        TestCase::new(
            "header_deny_allows_normal",
            "Header :  User-Agent  (200)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/test/header-deny/api", ctx.http_url());

                    let request = client
                        .get(&url)
                        .header("host", "request-restriction.example.com")
                        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0)");

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Normal UA passed with status {}", status),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected status 200, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 2. Method allow ====================
    fn test_method_allow_get() -> TestCase {
        TestCase::new("method_allow_get", "Method : GET  (200)", |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();
                let client = &ctx.http_client;
                let url = format!("{}/test/method-allow/api", ctx.http_url());

                let request = client.get(&url).header("host", "request-restriction.example.com");

                match request.send().await {
                    Ok(response) => {
                        let status = response.status().as_u16();
                        if status == 200 {
                            TestResult::passed_with_message(
                                start.elapsed(),
                                format!("GET passed with status {}", status),
                            )
                        } else {
                            TestResult::failed(start.elapsed(), format!("Expected status 200, got {}", status))
                        }
                    }
                    Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                }
            })
        })
    }

    fn test_method_allow_blocks_post() -> TestCase {
        TestCase::new(
            "method_allow_blocks_post",
            "Method : POST  (405)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/test/method-allow/api", ctx.http_url());

                    let request = client
                        .post(&url)
                        .header("host", "request-restriction.example.com")
                        .body("");

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 405 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("POST blocked with status {}", status),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected status 405, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 3. Header required ====================
    fn test_header_required_with_token() -> TestCase {
        TestCase::new(
            "header_required_with_token",
            "Header :  Token  (200)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/test/header-required/api", ctx.http_url());

                    let request = client
                        .get(&url)
                        .header("host", "request-restriction.example.com")
                        .header("X-Auth-Token", "valid-token-123");

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Valid token passed with status {}", status),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected status 200, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_header_required_missing() -> TestCase {
        TestCase::new(
            "header_required_missing",
            "Header :  Token  (401)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/test/header-required/api", ctx.http_url());

                    let request = client.get(&url).header("host", "request-restriction.example.com");
                    // No X-Auth-Token header sent.

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 401 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Missing token rejected with status {}", status),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected status 401, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 4. Combined rules ====================
    fn test_combined_normal() -> TestCase {
        TestCase::new("combined_normal", ":  (200)", |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();
                let client = &ctx.http_client;
                let url = format!("{}/test/combined/api/users", ctx.http_url());

                let request = client
                    .get(&url)
                    .header("host", "request-restriction.example.com")
                    .header("User-Agent", "Mozilla/5.0");

                match request.send().await {
                    Ok(response) => {
                        let status = response.status().as_u16();
                        if status == 200 {
                            TestResult::passed_with_message(
                                start.elapsed(),
                                format!("Normal request passed with status {}", status),
                            )
                        } else {
                            TestResult::failed(start.elapsed(), format!("Expected status 200, got {}", status))
                        }
                    }
                    Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                }
            })
        })
    }

    fn test_combined_bot_blocked() -> TestCase {
        TestCase::new("combined_bot_blocked", ": Bot UA  (403)", |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();
                let client = &ctx.http_client;
                let url = format!("{}/test/combined/api/users", ctx.http_url());

                let request = client
                    .get(&url)
                    .header("host", "request-restriction.example.com")
                    .header("User-Agent", "Googlebot/2.1");

                match request.send().await {
                    Ok(response) => {
                        let status = response.status().as_u16();
                        if status == 403 {
                            TestResult::passed_with_message(
                                start.elapsed(),
                                format!("Bot blocked with status {}", status),
                            )
                        } else {
                            TestResult::failed(start.elapsed(), format!("Expected status 403, got {}", status))
                        }
                    }
                    Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                }
            })
        })
    }

    // ==================== 5. Query source (unified-query-string-cache) ====================
    /// Query source decoded semantics: deny rule key "name" deny "hello world"
    /// must match the percent-encoded query "?name=hello%20world" because
    /// PluginSession::get_query_param returns the decoded value.
    fn test_query_deny_decoded() -> TestCase {
        TestCase::new(
            "query_deny_decoded",
            "Query source: percent-encoded query matches decoded deny value",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    // Positive: hello%20world decodes to "hello world" — must be denied.
                    let denied = ctx
                        .http_client
                        .get(format!("{}/test/query-deny?name=hello%20world", ctx.http_url()))
                        .header("Host", "request-restriction.example.com")
                        .send()
                        .await;
                    match denied {
                        Ok(r) if r.status() == 403 => {}
                        Ok(r) => {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Expected 403 for decoded match, got {}", r.status()),
                            )
                        }
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Denied request failed: {}", e)),
                    }
                    // Negative: different value — must pass.
                    let allowed = ctx
                        .http_client
                        .get(format!("{}/test/query-deny?name=foo", ctx.http_url()))
                        .header("Host", "request-restriction.example.com")
                        .send()
                        .await;
                    match allowed {
                        Ok(r) if r.status().is_success() => TestResult::passed_with_message(
                            start.elapsed(),
                            "Query source decoded matching: denied 'hello world', allowed foo".to_string(),
                        ),
                        Ok(r) => TestResult::failed(
                            start.elapsed(),
                            format!("Expected 2xx for non-match, got {}", r.status()),
                        ),
                        Err(e) => TestResult::failed(start.elapsed(), format!("Allowed request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 6. AND within a group ====================
    // Group = (X-Env: prod) AND (path matches /admin/). Both must hold to deny.

    fn test_and_group_both_match_blocked() -> TestCase {
        TestCase::new(
            "and_group_both_match_blocked",
            "AND group: prod header + admin path => denied (403)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/test/and-group/admin/users", ctx.http_url());
                    let request = ctx
                        .http_client
                        .get(&url)
                        .header("host", "request-restriction.example.com")
                        .header("X-Env", "prod");
                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 403 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Both predicates matched, blocked with status {}", status),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected status 403, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_and_group_header_mismatch_allowed() -> TestCase {
        TestCase::new(
            "and_group_header_mismatch_allowed",
            "AND group: admin path but non-prod header => passes (200)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/test/and-group/admin/users", ctx.http_url());
                    // Admin path matches, but X-Env != prod => the group's AND fails => pass.
                    let request = ctx
                        .http_client
                        .get(&url)
                        .header("host", "request-restriction.example.com")
                        .header("X-Env", "dev");
                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Header predicate false, passed with status {}", status),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected status 200, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_and_group_path_mismatch_allowed() -> TestCase {
        TestCase::new(
            "and_group_path_mismatch_allowed",
            "AND group: prod header but non-admin path => passes (200)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/test/and-group/public/users", ctx.http_url());
                    // X-Env matches, but path is not /admin/ => the group's AND fails => pass.
                    let request = ctx
                        .http_client
                        .get(&url)
                        .header("host", "request-restriction.example.com")
                        .header("X-Env", "prod");
                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Path predicate false, passed with status {}", status),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected status 200, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 7. JwtClaim source (JwtAuth -> RequestRestriction chain) ====================
    // The route runs JwtAuth (storeClaimsInCtx) first, then RequestRestriction in
    // mode: Allow on the verified claim realm_access.roles. A token whose roles array
    // contains "admin" is admitted ("any element matches"); anything else is denied 403.

    fn test_jwt_claim_admin_allowed() -> TestCase {
        TestCase::new(
            "jwt_claim_admin_allowed",
            "JwtClaim: roles array contains admin => allowed (200)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let jwt = build_jwt_with_roles(&["viewer", "admin"]);
                    let url = format!("{}/test/jwt-claim/api", ctx.http_url());
                    let request = ctx
                        .http_client
                        .get(&url)
                        .header("host", RR_HOST)
                        .header("Authorization", format!("Bearer {}", jwt));
                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("admin role admitted with status {}", status),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected status 200, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_jwt_claim_non_admin_denied() -> TestCase {
        TestCase::new(
            "jwt_claim_non_admin_denied",
            "JwtClaim: roles array without admin => denied (403)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let jwt = build_jwt_with_roles(&["viewer", "editor"]);
                    let url = format!("{}/test/jwt-claim/api", ctx.http_url());
                    let request = ctx
                        .http_client
                        .get(&url)
                        .header("host", RR_HOST)
                        .header("Authorization", format!("Bearer {}", jwt));
                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 403 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("non-admin roles denied with status {}", status),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected status 403, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_jwt_claim_missing_claim_denied() -> TestCase {
        TestCase::new(
            "jwt_claim_missing_claim_denied",
            "JwtClaim: token without realm_access => missing claim is false => denied (403)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let jwt = build_jwt_without_roles();
                    let url = format!("{}/test/jwt-claim/api", ctx.http_url());
                    let request = ctx
                        .http_client
                        .get(&url)
                        .header("host", RR_HOST)
                        .header("Authorization", format!("Bearer {}", jwt));
                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 403 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("missing claim denied with status {}", status),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected status 403, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_jwt_claim_no_token_401() -> TestCase {
        TestCase::new(
            "jwt_claim_no_token_401",
            "JwtClaim chain: no token => JwtAuth short-circuits (401)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/test/jwt-claim/api", ctx.http_url());
                    let request = ctx.http_client.get(&url).header("host", RR_HOST);
                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 401 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("no token rejected by JwtAuth with status {}", status),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected status 401, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_combined_admin_blocked() -> TestCase {
        TestCase::new("combined_admin_blocked", ": Admin  (403)", |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();
                let client = &ctx.http_client;
                let url = format!("{}/test/combined/admin/users", ctx.http_url());

                let request = client
                    .get(&url)
                    .header("host", "request-restriction.example.com")
                    .header("User-Agent", "Mozilla/5.0");

                match request.send().await {
                    Ok(response) => {
                        let status = response.status().as_u16();
                        if status == 403 {
                            TestResult::passed_with_message(
                                start.elapsed(),
                                format!("Admin path blocked with status {}", status),
                            )
                        } else {
                            TestResult::failed(start.elapsed(), format!("Expected status 403, got {}", status))
                        }
                    }
                    Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                }
            })
        })
    }

    fn test_ip_and_path_admin_blocked() -> TestCase {
        TestCase::new(
            "ip_and_path_admin_blocked",
            "IP and path in one group: local peer + admin path => denied (403)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/test/ip-and-path/admin/users", ctx.http_url());
                    let request = ctx
                        .http_client
                        .get(&url)
                        .header("host", "request-restriction.example.com");
                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 403 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("local peer + admin path denied with status {}", status),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected status 403, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_ip_and_path_public_allowed() -> TestCase {
        TestCase::new(
            "ip_and_path_public_allowed",
            "IP and path in one group: local peer + public path => passes (200)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/test/ip-and-path/public/users", ctx.http_url());
                    let request = ctx
                        .http_client
                        .get(&url)
                        .header("host", "request-restriction.example.com");
                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("path predicate false, passed with status {}", status),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected status 200, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_iprefs_group_admin_blocked() -> TestCase {
        TestCase::new(
            "iprefs_group_admin_blocked",
            "ipRefs in RR group: local peer + admin path => denied (403)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/test/iprefs-group/admin/users", ctx.http_url());
                    let request = ctx.http_client.get(&url).header("host", RR_HOST);
                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 403 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("referenced IP list + admin path denied with status {}", status),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected status 403, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_iprefs_group_public_allowed() -> TestCase {
        TestCase::new(
            "iprefs_group_public_allowed",
            "ipRefs in RR group: local peer + public path => passes (200)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/test/iprefs-group/public/users", ctx.http_url());
                    let request = ctx.http_client.get(&url).header("host", RR_HOST);
                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("path predicate false, passed with status {}", status),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected status 200, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_iprefs_run_gated_blocked() -> TestCase {
        TestCase::new(
            "iprefs_run_gated_blocked",
            "entry-level run ipRefs: local peer sets header, gated path denied (403)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/test/iprefs-run/gated/users", ctx.http_url());
                    let request = ctx.http_client.get(&url).header("host", RR_HOST);
                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 403 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("entry-level ipRefs run denied gated path with status {}", status),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected status 403, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_iprefs_run_open_allowed() -> TestCase {
        TestCase::new(
            "iprefs_run_open_allowed",
            "entry-level run ipRefs: local peer + open path => passes (200)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/test/iprefs-run/open/users", ctx.http_url());
                    let request = ctx.http_client.get(&url).header("host", RR_HOST);
                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("gated path predicate false, passed with status {}", status),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected status 200, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_missing_ipref_deny_fails_closed() -> TestCase {
        TestCase::new(
            "missing_ipref_deny_fails_closed",
            "unresolved ipRefs in RequestRestriction Deny mode => generic 500",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/test/missing-ipref-deny/api", ctx.http_url());
                    match ctx.http_client.get(&url).header("host", RR_HOST).send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            let body = response.text().await.unwrap_or_default();
                            if status == 500 && body.contains("Internal restriction policy error") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "unresolved deny reference reached the restriction fail-closed path".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected restriction error 500, got status {status} body {body:?}"),
                                )
                            }
                        }
                        Err(error) => TestResult::failed(start.elapsed(), format!("Request failed: {error}")),
                    }
                })
            },
        )
    }

    fn test_missing_ipref_entry_run_seals_resource() -> TestCase {
        TestCase::new(
            "missing_ipref_entry_run_seals_resource",
            "unresolved ipRefs in entry-level run => resource returns 500",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/test/missing-ipref-run/api", ctx.http_url());
                    match ctx.http_client.get(&url).header("host", RR_HOST).send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            let body = response.text().await.unwrap_or_default();
                            if status == 500 && body.contains("plugin configuration error") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "unresolved run gate reached the sealed runtime".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected plugin configuration error 500, got status {status} body {body:?}"
                                    ),
                                )
                            }
                        }
                        Err(error) => TestResult::failed(start.elapsed(), format!("Request failed: {error}")),
                    }
                })
            },
        )
    }

    fn test_rr_hmac_valid_allowed() -> TestCase {
        TestCase::new(
            "rr_hmac_valid_allowed",
            "RR HMAC Allow: valid signature => passes (200)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let path = "/test/rr-hmac/api";
                    let signed = build_hmac_headers(RR_HMAC_USERNAME, RR_HMAC_SECRET, "GET", path, None, RR_HOST, None);
                    let url = format!("{}{path}", ctx.http_url());
                    let request = ctx
                        .http_client
                        .get(&url)
                        .header("host", RR_HOST)
                        .header("date", &signed.date)
                        .header("authorization", &signed.authorization);
                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("valid HMAC allowed with status {}", status),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected status 200, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_rr_hmac_unsigned_denied() -> TestCase {
        TestCase::new(
            "rr_hmac_unsigned_denied",
            "RR HMAC Allow: unsigned request => denied (403)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/test/rr-hmac/api", ctx.http_url());
                    let request = ctx.http_client.get(&url).header("host", RR_HOST);
                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 403 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("unsigned HMAC denied with status {}", status),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected status 403, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_rr_hmac_bad_signature_denied() -> TestCase {
        TestCase::new(
            "rr_hmac_bad_signature_denied",
            "RR HMAC Allow: bad signature => denied (403)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let path = "/test/rr-hmac/api";
                    let signed =
                        build_hmac_headers(RR_HMAC_USERNAME, RR_HMAC_BAD_SECRET, "GET", path, None, RR_HOST, None);
                    let url = format!("{}{path}", ctx.http_url());
                    let request = ctx
                        .http_client
                        .get(&url)
                        .header("host", RR_HOST)
                        .header("date", &signed.date)
                        .header("authorization", &signed.authorization);
                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 403 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("bad HMAC signature denied with status {}", status),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected status 403, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }
}

impl TestSuite for RequestRestrictionTestSuite {
    fn name(&self) -> &str {
        "RequestRestriction Plugin Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            // Header deny by value
            Self::test_header_deny_blocks_bot(),
            Self::test_header_deny_allows_normal(),
            // Method allow
            Self::test_method_allow_get(),
            Self::test_method_allow_blocks_post(),
            // Header required
            Self::test_header_required_with_token(),
            Self::test_header_required_missing(),
            // Combined rules
            Self::test_combined_normal(),
            Self::test_combined_bot_blocked(),
            Self::test_combined_admin_blocked(),
            // Query source decoded semantics (unified-query-string-cache)
            Self::test_query_deny_decoded(),
            // AND within a group (DNF rule-groups)
            Self::test_and_group_both_match_blocked(),
            Self::test_and_group_header_mismatch_allowed(),
            Self::test_and_group_path_mismatch_allowed(),
            // JwtClaim source (JwtAuth -> RequestRestriction chain)
            Self::test_jwt_claim_admin_allowed(),
            Self::test_jwt_claim_non_admin_denied(),
            Self::test_jwt_claim_missing_claim_denied(),
            Self::test_jwt_claim_no_token_401(),
            // IP and path in one group
            Self::test_ip_and_path_admin_blocked(),
            Self::test_ip_and_path_public_allowed(),
            // S14: ipRefs in a RR group and entry-level run
            Self::test_iprefs_group_admin_blocked(),
            Self::test_iprefs_group_public_allowed(),
            Self::test_iprefs_run_gated_blocked(),
            Self::test_iprefs_run_open_allowed(),
            // cl-01: unresolved references must not become negatable false.
            Self::test_missing_ipref_deny_fails_closed(),
            Self::test_missing_ipref_entry_run_seals_resource(),
            // S14: RR HMAC resolved credentials
            Self::test_rr_hmac_valid_allowed(),
            Self::test_rr_hmac_unsigned_denied(),
            Self::test_rr_hmac_bad_signature_denied(),
        ]
    }
}
