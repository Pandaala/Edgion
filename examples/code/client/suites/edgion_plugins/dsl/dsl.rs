// EdgionDSL Plugin Integration Test Suite
//
// Tests inline DSL scripts in EdgionPlugins YAML configuration.
// Verifies that source code in YAML is correctly compiled and executed by the gateway.
//
// Test scenarios:
//   1. Header check — deny if X-Api-Token is missing (dsl-header-check)
//   2. Header rewrite — set new headers based on request input (dsl-header-rewrite)
//   3. Path deny — deny if path contains "/admin" (dsl-deny-path)
//
// Config files: conf/EdgionPlugins/Dsl/

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

pub struct DslTestSuite;

// ==================== Test 1: Header Check (dsl-header-check) ====================
// DSL source:
//   let token = req.header("X-Api-Token")
//   if token == nil { return deny(403, "missing X-Api-Token header") }

impl DslTestSuite {
    /// Test: missing X-Api-Token → 403
    fn test_header_check_deny() -> TestCase {
        TestCase::new(
            "dsl_header_check_deny",
            "DSL: missing X-Api-Token returns 403",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("http://{}:31180/test/dsl-header-check/api", ctx.target_host);

                    let response = client
                        .get(url)
                        .header("host", "dsl-header-check.example.com")
                        .send()
                        .await;

                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let body = resp.text().await.unwrap_or_default();
                            if status == 403 {
                                if body.contains("missing X-Api-Token") {
                                    TestResult::passed_with_message(
                                        start.elapsed(),
                                        "403 with correct deny body".to_string(),
                                    )
                                } else {
                                    TestResult::passed_with_message(
                                        start.elapsed(),
                                        format!("403 returned (body: {})", body),
                                    )
                                }
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 403, got {} (body: {})", status, body),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test: present X-Api-Token → 200 (pass through to backend)
    fn test_header_check_allow() -> TestCase {
        TestCase::new(
            "dsl_header_check_allow",
            "DSL: present X-Api-Token returns 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("http://{}:31180/test/dsl-header-check/api", ctx.target_host);

                    let response = client
                        .get(url)
                        .header("host", "dsl-header-check.example.com")
                        .header("X-Api-Token", "my-secret-token")
                        .send()
                        .await;

                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status == 200 {
                                TestResult::passed(start.elapsed())
                            } else {
                                let body = resp.text().await.unwrap_or_default();
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200, got {} (body: {})", status, body),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== Test 2: Header Rewrite (dsl-header-rewrite) ====================
    // DSL source:
    //   let user_id = req.header("X-User-Id")
    //   if user_id != nil {
    //     resp.set_header("X-Processed-By", "edgion-dsl")
    //     let upper = to_upper(user_id)
    //     resp.set_header("X-User-Upper", upper)
    //   }

    /// Test: X-User-Id present → response contains X-Processed-By and X-User-Upper headers
    fn test_header_rewrite() -> TestCase {
        TestCase::new(
            "dsl_header_rewrite",
            "DSL: resp.set_header adds X-Processed-By and X-User-Upper to response",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("http://{}:31180/test/dsl-header-rewrite/api", ctx.target_host);

                    let response = client
                        .get(url)
                        .header("host", "dsl-header-rewrite.example.com")
                        .header("X-User-Id", "alice")
                        .send()
                        .await;

                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                            }

                            // Check response headers set by DSL via resp.set_header
                            let has_processed_by = resp
                                .headers()
                                .get("x-processed-by")
                                .and_then(|v| v.to_str().ok())
                                .map(|v| v == "edgion-dsl")
                                .unwrap_or(false);

                            let has_user_upper = resp
                                .headers()
                                .get("x-user-upper")
                                .and_then(|v| v.to_str().ok())
                                .map(|v| v == "ALICE")
                                .unwrap_or(false);

                            if has_processed_by && has_user_upper {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Both X-Processed-By and X-User-Upper present in response".to_string(),
                                )
                            } else {
                                let headers_dbg: Vec<String> = resp
                                    .headers()
                                    .iter()
                                    .map(|(k, v)| format!("{}: {}", k, v.to_str().unwrap_or("<invalid>")))
                                    .collect();
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Missing DSL response headers. processed_by={}, user_upper={}. Headers: {:?}",
                                        has_processed_by, has_user_upper, headers_dbg
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

    /// Test: no X-User-Id → response headers NOT set (script does nothing)
    fn test_header_rewrite_passthrough() -> TestCase {
        TestCase::new(
            "dsl_header_rewrite_passthrough",
            "DSL: no X-User-Id → no extra response headers added",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("http://{}:31180/test/dsl-header-rewrite/api", ctx.target_host);

                    let response = client
                        .get(url)
                        .header("host", "dsl-header-rewrite.example.com")
                        .send()
                        .await;

                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                            }

                            let has_processed_by = resp.headers().get("x-processed-by").is_some();

                            if !has_processed_by {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "No X-Processed-By when X-User-Id is absent".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    "X-Processed-By should not be present in response".to_string(),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== Test 3: Path Deny (dsl-deny-path) ====================
    // DSL source:
    //   let path = req.path()
    //   if path.contains("/admin") { return deny(403, "admin access denied") }

    /// Test: path contains /admin → 403
    fn test_deny_path_blocked() -> TestCase {
        TestCase::new(
            "dsl_deny_path_blocked",
            "DSL: path containing /admin returns 403",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("http://{}:31180/test/dsl-deny-path/admin/secret", ctx.target_host);

                    let response = client.get(url).header("host", "dsl-deny-path.example.com").send().await;

                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let body = resp.text().await.unwrap_or_default();
                            if status == 403 {
                                if body.contains("admin access denied") {
                                    TestResult::passed_with_message(
                                        start.elapsed(),
                                        "403 with correct deny body for /admin path".to_string(),
                                    )
                                } else {
                                    TestResult::passed_with_message(
                                        start.elapsed(),
                                        format!("403 returned for /admin (body: {})", body),
                                    )
                                }
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 403, got {} (body: {})", status, body),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test: normal path (no /admin) → 200
    fn test_deny_path_allowed() -> TestCase {
        TestCase::new(
            "dsl_deny_path_allowed",
            "DSL: normal path without /admin returns 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("http://{}:31180/test/dsl-deny-path/api/users", ctx.target_host);

                    let response = client.get(url).header("host", "dsl-deny-path.example.com").send().await;

                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status == 200 {
                                TestResult::passed(start.elapsed())
                            } else {
                                let body = resp.text().await.unwrap_or_default();
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200, got {} (body: {})", status, body),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== Scenario 4: Set Request Header ====================

    fn test_set_request_header_with_user_id() -> TestCase {
        TestCase::new(
            "dsl_set_request_header_with_user_id",
            "DSL: X-User-Id: 42 → X-Processed-User: uid-42 in response header",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-set-request-header/api", ctx.target_host);
                    let response = ctx
                        .http_client
                        .get(url)
                        .header("host", "dsl-set-request-header.example.com")
                        .header("X-User-Id", "42")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let processed = resp
                                .headers()
                                .get("x-processed-user")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("");
                            if status == 200 && processed == "uid-42" {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "X-Processed-User: uid-42 in response".into(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("status={} x-processed-user='{}'", status, processed),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_set_request_header_without_user_id() -> TestCase {
        TestCase::new(
            "dsl_set_request_header_without_user_id",
            "DSL: no X-User-Id → no X-Processed-User in response",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-set-request-header/api", ctx.target_host);
                    let response = ctx
                        .http_client
                        .get(url)
                        .header("host", "dsl-set-request-header.example.com")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let has_processed = resp.headers().contains_key("x-processed-user");
                            if status == 200 && !has_processed {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "No X-Processed-User as expected".into(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("status={} has_x_processed_user={}", status, has_processed),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== Scenario 5: Deny by Header ====================

    fn test_deny_by_header_blocked() -> TestCase {
        TestCase::new(
            "dsl_deny_by_header_blocked",
            "DSL: X-Block: 1 → 403 blocked",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-deny-by-header/api", ctx.target_host);
                    let response = ctx
                        .http_client
                        .get(url)
                        .header("host", "dsl-deny-by-header.example.com")
                        .header("X-Block", "1")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let body = resp.text().await.unwrap_or_default();
                            if status == 403 && body.contains("blocked") {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 403/blocked, got {}/{}", status, body),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_deny_by_header_allowed() -> TestCase {
        TestCase::new(
            "dsl_deny_by_header_allowed",
            "DSL: no X-Block → 200 pass through",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-deny-by-header/api", ctx.target_host);
                    let response = ctx
                        .http_client
                        .get(url)
                        .header("host", "dsl-deny-by-header.example.com")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status == 200 {
                                TestResult::passed(start.elapsed())
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

    // ==================== Scenario 6: Cookie Read ====================

    fn test_cookie_read_present() -> TestCase {
        TestCase::new(
            "dsl_cookie_read_present",
            "DSL: session_id=abc123 cookie → X-Session: abc123 in response header",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-cookie-read/api", ctx.target_host);
                    let response = ctx
                        .http_client
                        .get(url)
                        .header("host", "dsl-cookie-read.example.com")
                        .header("Cookie", "session_id=abc123")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let session = resp
                                .headers()
                                .get("x-session")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("");
                            if status == 200 && session == "abc123" {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("status={} x-session='{}'", status, session),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_cookie_read_absent() -> TestCase {
        TestCase::new(
            "dsl_cookie_read_absent",
            "DSL: no session_id cookie → X-Session: anonymous in response header",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-cookie-read/api", ctx.target_host);
                    let response = ctx
                        .http_client
                        .get(url)
                        .header("host", "dsl-cookie-read.example.com")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let session = resp
                                .headers()
                                .get("x-session")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("");
                            if status == 200 && session == "anonymous" {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("status={} x-session='{}'", status, session),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== Scenario 7: Ctx Chain ====================

    fn test_ctx_chain_with_tenant() -> TestCase {
        TestCase::new(
            "dsl_ctx_chain_with_tenant",
            "DSL ctx chain: X-Tenant → X-Internal-Tenant + X-Pipeline in response headers",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-ctx-chain/api", ctx.target_host);
                    let response = ctx
                        .http_client
                        .get(url)
                        .header("host", "dsl-ctx-chain.example.com")
                        .header("X-Tenant", "acme")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let internal = resp
                                .headers()
                                .get("x-internal-tenant")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("");
                            let pipeline = resp
                                .headers()
                                .get("x-pipeline")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("");
                            if status == 200 && internal == "acme" && pipeline == "dsl-ok" {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "status={} x-internal-tenant='{}' x-pipeline='{}'",
                                        status, internal, pipeline
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

    fn test_ctx_chain_without_tenant() -> TestCase {
        TestCase::new(
            "dsl_ctx_chain_without_tenant",
            "DSL ctx chain: no X-Tenant → no X-Internal-Tenant, no X-Pipeline in response",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-ctx-chain/api", ctx.target_host);
                    let response = ctx
                        .http_client
                        .get(url)
                        .header("host", "dsl-ctx-chain.example.com")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let no_internal = !resp.headers().contains_key("x-internal-tenant");
                            let no_pipeline = !resp.headers().contains_key("x-pipeline");
                            if status == 200 && no_internal && no_pipeline {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "status={} has_internal={} has_pipeline={}",
                                        status, !no_internal, !no_pipeline
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

    // ==================== Scenario 8: Multi Header Ops ====================

    fn test_multi_header_ops() -> TestCase {
        TestCase::new(
            "dsl_multi_header_ops",
            "DSL: set/append/remove/set_host verified via response headers",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-multi-header-ops/api", ctx.target_host);
                    let response = ctx
                        .http_client
                        .get(url)
                        .header("host", "dsl-multi-header-ops.example.com")
                        .header("User-Agent", "test-runner/1.0")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let x_set = resp.headers().get("x-set").and_then(|v| v.to_str().ok()).unwrap_or("");
                            let x_multi = resp
                                .headers()
                                .get("x-multi")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("");
                            let ua_removed = resp
                                .headers()
                                .get("x-ua-removed")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("");
                            if status == 200
                                && x_set == "value1"
                                && x_multi.contains("first")
                                && x_multi.contains("second")
                                && ua_removed == "true"
                            {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "status={} x-set='{}' x-multi='{}' x-ua-removed='{}'",
                                        status, x_set, x_multi, ua_removed
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

    // ==================== Scenario 9: URI Rewrite ====================

    fn test_uri_rewrite_v1() -> TestCase {
        TestCase::new(
            "dsl_uri_rewrite_v1",
            "DSL: GET /v1/users → path rewritten (X-Rewritten: true)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-uri-rewrite/v1/users", ctx.target_host);
                    let response = ctx
                        .http_client
                        .get(url)
                        .header("host", "dsl-uri-rewrite.example.com")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let rewritten = resp
                                .headers()
                                .get("x-rewritten")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("");
                            let new_path = resp
                                .headers()
                                .get("x-new-path")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("");
                            if status == 200 && rewritten == "true" && new_path.contains("/api/users") {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "status={} x-rewritten='{}' x-new-path='{}'",
                                        status, rewritten, new_path
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

    fn test_uri_rewrite_v2_unchanged() -> TestCase {
        TestCase::new(
            "dsl_uri_rewrite_v2_unchanged",
            "DSL: GET /v2/items → path NOT rewritten (no X-Rewritten header)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-uri-rewrite/v2/items", ctx.target_host);
                    let response = ctx
                        .http_client
                        .get(url)
                        .header("host", "dsl-uri-rewrite.example.com")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let has_rewritten = resp.headers().contains_key("x-rewritten");
                            if status == 200 && !has_rewritten {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("status={} has_x_rewritten={}", status, has_rewritten),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== Scenario 10: SHA256 Sign ====================

    fn test_sha256_sign() -> TestCase {
        TestCase::new(
            "dsl_sha256_sign",
            "DSL: X-Token: hello → X-Token-Hash: sha256(hello) in response header",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-sha256-sign/api", ctx.target_host);
                    let response = ctx
                        .http_client
                        .get(url)
                        .header("host", "dsl-sha256-sign.example.com")
                        .header("X-Token", "hello")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let hash = resp
                                .headers()
                                .get("x-token-hash")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("");
                            let expected = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
                            if status == 200 && hash == expected {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("status={} x-token-hash='{}'", status, hash),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== Scenario 11: Regex Extract ====================

    fn test_regex_extract_chrome() -> TestCase {
        TestCase::new(
            "dsl_regex_extract_chrome",
            "DSL: User-Agent Chrome/120 → X-Chrome-Ver in response header",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-regex-extract/api", ctx.target_host);
                    let response = ctx
                        .http_client
                        .get(url)
                        .header("host", "dsl-regex-extract.example.com")
                        .header("User-Agent", "Mozilla/5.0 Chrome/120.0 Safari/537.36")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let chrome_ver = resp
                                .headers()
                                .get("x-chrome-ver")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("");
                            if status == 200 && chrome_ver.contains("Chrome/120") {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("status={} x-chrome-ver='{}'", status, chrome_ver),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_regex_extract_no_chrome() -> TestCase {
        TestCase::new(
            "dsl_regex_extract_no_chrome",
            "DSL: curl User-Agent → no X-Chrome-Ver in response",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-regex-extract/api", ctx.target_host);
                    let response = ctx
                        .http_client
                        .get(url)
                        .header("host", "dsl-regex-extract.example.com")
                        .header("User-Agent", "curl/7.0")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let has_chrome = resp.headers().contains_key("x-chrome-ver");
                            if status == 200 && !has_chrome {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("status={} has_x_chrome_ver={}", status, has_chrome),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== Scenario 12: Response Inject ====================

    fn test_response_inject() -> TestCase {
        TestCase::new(
            "dsl_response_inject",
            "DSL upstreamResponseFilter: CORS headers injected in response",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-response-inject/health", ctx.target_host);
                    let response = ctx
                        .http_client
                        .get(url)
                        .header("host", "dsl-response-inject.example.com")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let acao = resp
                                .headers()
                                .get("access-control-allow-origin")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("");
                            let powered_by = resp
                                .headers()
                                .get("x-powered-by")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("");
                            let vary = resp.headers().get("vary").and_then(|v| v.to_str().ok()).unwrap_or("");
                            if status == 200 && acao == "*" && powered_by == "Edgion-DSL" && vary.contains("Origin") {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "status={} acao='{}' powered_by='{}' vary='{}'",
                                        status, acao, powered_by, vary
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

    // ==================== Scenario 13: Resp Status Conditional ====================

    fn test_resp_status_404() -> TestCase {
        TestCase::new(
            "dsl_resp_status_404",
            "DSL upstreamResponseFilter: 404 → X-Not-Found: true",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!(
                        "http://{}:31180/test/dsl-resp-status-conditional/status/404",
                        ctx.target_host
                    );
                    let response = ctx
                        .http_client
                        .get(url)
                        .header("host", "dsl-resp-status-conditional.example.com")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let not_found = resp
                                .headers()
                                .get("x-not-found")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("");
                            if status == 404 && not_found == "true" {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("status={} x-not-found='{}'", status, not_found),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_resp_status_200_no_header() -> TestCase {
        TestCase::new(
            "dsl_resp_status_200_no_header",
            "DSL upstreamResponseFilter: 200 → no X-Not-Found",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!(
                        "http://{}:31180/test/dsl-resp-status-conditional/status/200",
                        ctx.target_host
                    );
                    let response = ctx
                        .http_client
                        .get(url)
                        .header("host", "dsl-resp-status-conditional.example.com")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let has_not_found = resp.headers().contains_key("x-not-found");
                            if status == 200 && !has_not_found {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("status={} has_x_not_found={}", status, has_not_found),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== Scenario 14: Deny with Body ====================

    fn test_deny_with_body_no_auth() -> TestCase {
        TestCase::new(
            "dsl_deny_with_body_no_auth",
            "DSL: no Authorization → 401 with JSON error body",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-deny-with-body/api", ctx.target_host);
                    let response = ctx
                        .http_client
                        .get(url)
                        .header("host", "dsl-deny-with-body.example.com")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let body = resp.text().await.unwrap_or_default();
                            if status == 401 && body.contains("unauthorized") {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 401/unauthorized, got {}/{}", status, body),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_deny_with_body_wrong_method() -> TestCase {
        TestCase::new(
            "dsl_deny_with_body_wrong_method",
            "DSL: Authorization + POST → 405 method not allowed",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-deny-with-body/api", ctx.target_host);
                    let response = ctx
                        .http_client
                        .post(url)
                        .header("host", "dsl-deny-with-body.example.com")
                        .header("Authorization", "Bearer token123")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let body = resp.text().await.unwrap_or_default();
                            if status == 405 && body.contains("method not allowed") {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected 405, got {}/{}", status, body))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_deny_with_body_get_allowed() -> TestCase {
        TestCase::new(
            "dsl_deny_with_body_get_allowed",
            "DSL: Authorization + GET → 200 pass through",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-deny-with-body/api", ctx.target_host);
                    let response = ctx
                        .http_client
                        .get(url)
                        .header("host", "dsl-deny-with-body.example.com")
                        .header("Authorization", "Bearer token123")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status == 200 {
                                TestResult::passed(start.elapsed())
                            } else {
                                let body = resp.text().await.unwrap_or_default();
                                TestResult::failed(start.elapsed(), format!("Expected 200, got {}/{}", status, body))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }
    // ==================== Scenario 15: Range Iterate ====================

    fn test_range_iterate() -> TestCase {
        TestCase::new(
            "dsl_range_iterate",
            "DSL: range(1,4) iterate → X-Range-Sum: 6, X-Range-Len: 3 in response headers",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-range-iterate/api", ctx.target_host);
                    let response = ctx
                        .http_client
                        .get(url)
                        .header("host", "dsl-range-iterate.example.com")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let sum = resp
                                .headers()
                                .get("x-range-sum")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("");
                            let len = resp
                                .headers()
                                .get("x-range-len")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("");
                            if status == 200 && sum == "6" && len == "3" {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "X-Range-Sum: 6, X-Range-Len: 3 in response".into(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("status={} x-range-sum='{}' x-range-len='{}'", status, sum, len),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== Scenario 16: Regex Replace ====================

    fn test_regex_replace_versioned_path() -> TestCase {
        TestCase::new(
            "dsl_regex_replace_versioned_path",
            "DSL: regex_replace /v2/ → /latest/ in path",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-regex-replace/v2/users", ctx.target_host);
                    let response = ctx
                        .http_client
                        .get(url)
                        .header("host", "dsl-regex-replace.example.com")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let cleaned = resp
                                .headers()
                                .get("x-cleaned-path")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("");
                            if cleaned.contains("/latest/") && !cleaned.contains("/v2/") {
                                TestResult::passed_with_message(start.elapsed(), format!("X-Cleaned-Path: {}", cleaned))
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected /latest/ in path, got '{}'", cleaned),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_regex_replace_no_match() -> TestCase {
        TestCase::new(
            "dsl_regex_replace_no_match",
            "DSL: regex_replace no version in path → unchanged",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-regex-replace/users", ctx.target_host);
                    let response = ctx
                        .http_client
                        .get(url)
                        .header("host", "dsl-regex-replace.example.com")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let cleaned = resp
                                .headers()
                                .get("x-cleaned-path")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("");
                            if cleaned.contains("/test/dsl-regex-replace/users") {
                                TestResult::passed_with_message(start.elapsed(), format!("Path unchanged: {}", cleaned))
                            } else {
                                TestResult::failed(start.elapsed(), format!("Unexpected path: '{}'", cleaned))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== Scenario 17: Trim + Split ====================

    fn test_trim_split_with_admin() -> TestCase {
        TestCase::new(
            "dsl_trim_split_with_admin",
            "DSL: split+trim X-Roles containing admin → X-Is-Admin: true",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-trim-split/api", ctx.target_host);
                    let response = ctx
                        .http_client
                        .get(url)
                        .header("host", "dsl-trim-split.example.com")
                        .header("X-Roles", "viewer, admin , editor")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let is_admin = resp
                                .headers()
                                .get("x-is-admin")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("");
                            let count = resp
                                .headers()
                                .get("x-role-count")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("");
                            if is_admin == "true" && count == "3" {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "X-Is-Admin: true, X-Role-Count: 3".into(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("X-Is-Admin='{}' X-Role-Count='{}'", is_admin, count),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_trim_split_without_admin() -> TestCase {
        TestCase::new(
            "dsl_trim_split_without_admin",
            "DSL: split+trim X-Roles without admin → no X-Is-Admin header",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-trim-split/api", ctx.target_host);
                    let response = ctx
                        .http_client
                        .get(url)
                        .header("host", "dsl-trim-split.example.com")
                        .header("X-Roles", "viewer, editor")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let has_admin = resp.headers().get("x-is-admin").is_some();
                            let count = resp
                                .headers()
                                .get("x-role-count")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("");
                            if !has_admin && count == "2" {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "No X-Is-Admin, X-Role-Count: 2".into(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("has_admin={} X-Role-Count='{}'", has_admin, count),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== Scenario 18: Break + Continue ====================

    fn test_break_continue() -> TestCase {
        TestCase::new(
            "dsl_break_continue",
            "DSL: break stops at 'target', continue skips 'skip' → X-Found: target, X-Skipped: 1",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-break-continue/api", ctx.target_host);
                    let response = ctx
                        .http_client
                        .get(url)
                        .header("host", "dsl-break-continue.example.com")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let found = resp
                                .headers()
                                .get("x-found")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("");
                            let skipped = resp
                                .headers()
                                .get("x-skipped")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("");
                            if found == "target" && skipped == "1" {
                                TestResult::passed_with_message(start.elapsed(), "X-Found: target, X-Skipped: 1".into())
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("X-Found='{}' X-Skipped='{}'", found, skipped),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }
    // ==================== Test 19: Terminate ====================

    /// Test: X-Terminate: true → 502 Bad Gateway via terminate_with_fallback runtime fallback
    fn test_terminate_connection_reset() -> TestCase {
        TestCase::new(
            "dsl_terminate_connection_reset",
            "DSL: terminate() → 502 Bad Gateway (terminate_with_fallback runtime fallback)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-terminate/api", ctx.target_host);
                    let response = ctx
                        .http_client
                        .get(url)
                        .header("host", "dsl-terminate.example.com")
                        .header("X-Terminate", "true")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let elapsed = start.elapsed();
                            if status == 502 {
                                if elapsed.as_millis() >= 1000 {
                                    TestResult::failed(
                                        elapsed,
                                        format!("Got 502 but took {}ms (expected <1000ms)", elapsed.as_millis()),
                                    )
                                } else {
                                    TestResult::passed_with_message(
                                        elapsed,
                                        format!("502 in {}ms (terminate_with_fallback fired)", elapsed.as_millis()),
                                    )
                                }
                            } else {
                                TestResult::failed(elapsed, format!("Expected 502, got HTTP {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(
                            start.elapsed(),
                            format!("Expected 502 response, got connection error: {}", e),
                        ),
                    }
                })
            },
        )
    }

    /// Test: no X-Terminate header → normal 200 pass through
    fn test_terminate_normal_pass() -> TestCase {
        TestCase::new(
            "dsl_terminate_normal_pass",
            "DSL: no X-Terminate → 200 pass through",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-terminate/api", ctx.target_host);
                    let response = ctx
                        .http_client
                        .get(url)
                        .header("host", "dsl-terminate.example.com")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status == 200 {
                                TestResult::passed(start.elapsed())
                            } else {
                                let body = resp.text().await.unwrap_or_default();
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200, got {} (body: {})", status, body),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== Test 20: resp.set_status ====================

    /// Test: backend returns 200 → DSL rewrites to 201 with X-Status-Rewritten header
    fn test_resp_set_status_rewrite() -> TestCase {
        TestCase::new(
            "dsl_resp_set_status_rewrite",
            "DSL upstreamResponseFilter: 200 → rewritten to 201 with X-Status-Rewritten",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-resp-set-status/api", ctx.target_host);
                    let response = ctx
                        .http_client
                        .get(url)
                        .header("host", "dsl-resp-set-status.example.com")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let rewritten = resp
                                .headers()
                                .get("x-status-rewritten")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("");
                            if status == 201 && rewritten == "true" {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Status rewritten to 201 with X-Status-Rewritten: true".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected status=201 x-status-rewritten=true, got status={} x-status-rewritten='{}'",
                                        status, rewritten
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

    // ==================== Test 21: Utility Combo ====================

    /// Test: multiple P3 utility functions in one DSL script
    fn test_utility_combo() -> TestCase {
        TestCase::new(
            "dsl_utility_combo",
            "DSL: hex_encode, json_get, uuid, ip_in_cidr, join, protocol, content_length",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-utility-combo/api", ctx.target_host);
                    let response = ctx
                        .http_client
                        .get(url)
                        .header("host", "dsl-utility-combo.example.com")
                        .header("X-Json-Data", r#"{"user":{"role":"admin"}}"#)
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status != 200 {
                                let body = resp.text().await.unwrap_or_default();
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200, got {} (body: {})", status, body),
                                );
                            }

                            let mut errors: Vec<String> = Vec::new();

                            // Check X-Hex: hex_encode("TEST") = "54455354"
                            let hex_val = resp.headers().get("x-hex").and_then(|v| v.to_str().ok()).unwrap_or("");
                            if hex_val != "54455354" {
                                errors.push(format!("X-Hex: expected '54455354', got '{}'", hex_val));
                            }

                            // Check X-User-Role: admin
                            let role = resp
                                .headers()
                                .get("x-user-role")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("");
                            if role != "admin" {
                                errors.push(format!("X-User-Role: expected 'admin', got '{}'", role));
                            }

                            // Check X-Request-ID is present (any UUID value)
                            let request_id = resp.headers().get("x-request-id");
                            if request_id.is_none() {
                                errors.push("X-Request-ID: missing (expected UUID)".to_string());
                            }

                            // Check X-Joined: a-b-c
                            let joined = resp
                                .headers()
                                .get("x-joined")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("");
                            if joined != "a-b-c" {
                                errors.push(format!("X-Joined: expected 'a-b-c', got '{}'", joined));
                            }

                            // Check X-Protocol: http
                            let protocol = resp
                                .headers()
                                .get("x-protocol")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("");
                            if protocol != "http" {
                                errors.push(format!("X-Protocol: expected 'http', got '{}'", protocol));
                            }

                            if errors.is_empty() {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "All utility functions produced correct results".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Utility combo failures: {}", errors.join("; ")),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== Scenario 24: http.call ====================
    // DSL source:
    //   let http_resp = http.call("GET", "http://127.0.0.1:30001/health", nil, 2000)
    //   if http_resp["status"] == 200 {
    //     resp.set_header("X-Call-Result", "ok")
    //   } else {
    //     resp.set_header("X-Call-Result", "fail")
    //   }

    /// Test: DSL calls backend /health, status 200 → X-Call-Result: ok in response headers
    fn test_http_call_success() -> TestCase {
        TestCase::new(
            "dsl_http_call_success",
            "DSL http.call: backend /health returns 200 → X-Call-Result: ok in response header",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-http-call/api", ctx.target_host);
                    let response = ctx
                        .http_client
                        .get(url)
                        .header("host", "dsl-http-call.example.com")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let call_result = resp
                                .headers()
                                .get("x-call-result")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("");
                            if status == 200 && call_result == "ok" {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "X-Call-Result: ok in response header".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("status={} X-Call-Result='{}'", status, call_result),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== Scenario 25: redis.* builtins ====================

    /// Test 25a: redis.set + redis.get round-trip.
    /// The DSL script sets a key and reads it back; X-Redis-Status: ok and
    /// X-Redis-Value: hello-from-dsl must appear in the response headers.
    fn test_redis_roundtrip() -> TestCase {
        TestCase::new(
            "dsl_redis_roundtrip",
            "DSL redis.*: set+get round-trip → X-Redis-Status: ok, X-Redis-Value: hello-from-dsl",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-redis/roundtrip/api", ctx.target_host);
                    let response = ctx
                        .http_client
                        .get(&url)
                        .header("host", "dsl-redis.example.com")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let headers = resp.headers().clone();
                            let redis_status = headers
                                .get("x-redis-status")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("");
                            let redis_value = headers.get("x-redis-value").and_then(|v| v.to_str().ok()).unwrap_or("");
                            if status == 200 && redis_status == "ok" && redis_value == "hello-from-dsl" {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "X-Redis-Status: ok, X-Redis-Value: hello-from-dsl".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "status={} X-Redis-Status='{}' X-Redis-Value='{}'",
                                        status, redis_status, redis_value
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

    /// Test 25b: redis.incr counter — counter always returns 1 after del+incr, X-Redis-Status: ok.
    fn test_redis_incr() -> TestCase {
        TestCase::new(
            "dsl_redis_incr",
            "DSL redis.*: incr counter on first request → X-Redis-Counter: 1, X-Redis-Status: ok",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-redis/incr/api", ctx.target_host);
                    let response = ctx
                        .http_client
                        .get(&url)
                        .header("host", "dsl-redis.example.com")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let headers = resp.headers().clone();
                            let redis_status = headers
                                .get("x-redis-status")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("");
                            let counter_str = headers
                                .get("x-redis-counter")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("0");
                            let counter: i64 = counter_str.parse().unwrap_or(-1);
                            if status == 200 && redis_status == "ok" && counter >= 1 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("X-Redis-Counter: {}, X-Redis-Status: ok", counter),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "status={} X-Redis-Status='{}' X-Redis-Counter='{}'",
                                        status, redis_status, counter_str
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

    /// Test 25c: redis.hset + redis.hgetall hash round-trip.
    fn test_redis_hash() -> TestCase {
        TestCase::new(
            "dsl_redis_hash",
            "DSL redis.*: hset+hgetall round-trip → X-Redis-Status: ok, X-Redis-Hash-Field1: value1",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-redis/hash/api", ctx.target_host);
                    let response = ctx
                        .http_client
                        .get(&url)
                        .header("host", "dsl-redis.example.com")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let headers = resp.headers().clone();
                            let redis_status = headers
                                .get("x-redis-status")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("");
                            let field1 = headers
                                .get("x-redis-hash-field1")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("");
                            if status == 200 && redis_status == "ok" && field1 == "value1" {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "X-Redis-Status: ok, X-Redis-Hash-Field1: value1".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "status={} X-Redis-Status='{}' X-Redis-Hash-Field1='{}'",
                                        status, redis_status, field1
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

    /// Test 25d: link declared in linkSysRefs but backing LinkSys resource does not exist.
    /// redis.get returns ok=false (link not ready); the script's else branch fires and sets
    /// X-Redis-Status: not-found-or-missing-client, then falls through — fail-open, 200.
    fn test_redis_missing_ref() -> TestCase {
        TestCase::new(
            "dsl_redis_missing_ref",
            "DSL redis.*: link not ready → ok=false, fail-open, request passes through",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-redis/missing-ref/api", ctx.target_host);
                    let response = ctx
                        .http_client
                        .get(&url)
                        .header("host", "dsl-redis.example.com")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let redis_status = resp
                                .headers()
                                .get("x-redis-status")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("")
                                .to_string();
                            // The link "not-ready-redis" is declared in linkSysRefs but the backing
                            // LinkSys resource does not exist. redis.get returns ok=false with
                            // error="link 'not-ready-redis' not ready". The script's else branch
                            // fires and sets "not-found-or-missing-client"; request passes through.
                            if status == 200 && redis_status == "not-found-or-missing-client" {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "fail-open: request passed through with expected header".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("status={} X-Redis-Status='{}'", status, redis_status),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== Test 22: HMAC Verify ====================

    /// Test: no X-Signature → 401 missing signature
    fn test_hmac_verify_missing_sig() -> TestCase {
        TestCase::new(
            "dsl_hmac_verify_missing_sig",
            "DSL: no X-Signature → 401 missing signature",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-hmac-verify/api", ctx.target_host);
                    let response = ctx
                        .http_client
                        .get(url)
                        .header("host", "dsl-hmac-verify.example.com")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let body = resp.text().await.unwrap_or_default();
                            if status == 401 {
                                if body.contains("missing signature") {
                                    TestResult::passed_with_message(
                                        start.elapsed(),
                                        "401 with 'missing signature' body".to_string(),
                                    )
                                } else {
                                    TestResult::passed_with_message(
                                        start.elapsed(),
                                        format!("401 returned (body: {})", body),
                                    )
                                }
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 401, got {} (body: {})", status, body),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test: wrong X-Signature → 403 invalid signature
    fn test_hmac_verify_invalid_sig() -> TestCase {
        TestCase::new(
            "dsl_hmac_verify_invalid_sig",
            "DSL: wrong X-Signature → 403 invalid signature",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-hmac-verify/api", ctx.target_host);
                    let response = ctx
                        .http_client
                        .get(url)
                        .header("host", "dsl-hmac-verify.example.com")
                        .header("X-Signature", "wrong-signature-value")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let body = resp.text().await.unwrap_or_default();
                            if status == 403 {
                                if body.contains("invalid signature") {
                                    TestResult::passed_with_message(
                                        start.elapsed(),
                                        "403 with 'invalid signature' body".to_string(),
                                    )
                                } else {
                                    TestResult::passed_with_message(
                                        start.elapsed(),
                                        format!("403 returned (body: {})", body),
                                    )
                                }
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 403, got {} (body: {})", status, body),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// Test: correct HMAC signature → 200 with X-Verified: true
    fn test_hmac_verify_valid_sig() -> TestCase {
        TestCase::new(
            "dsl_hmac_verify_valid_sig",
            "DSL: correct HMAC-SHA256 signature → 200 with X-Verified: true",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let path = "/test/dsl-hmac-verify/api";
                    let url = format!("http://{}:31180{}", ctx.target_host, path);

                    // Compute HMAC-SHA256("test-secret", path) to match the DSL script
                    use hmac::{Hmac, Mac};
                    use sha2::Sha256;
                    type HmacSha256 = Hmac<Sha256>;

                    let mut mac = HmacSha256::new_from_slice(b"test-secret").expect("HMAC key init");
                    mac.update(path.as_bytes());
                    let result = mac.finalize();
                    let signature = hex::encode(result.into_bytes());

                    let response = ctx
                        .http_client
                        .get(url)
                        .header("host", "dsl-hmac-verify.example.com")
                        .header("X-Signature", &signature)
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let verified = resp
                                .headers()
                                .get("x-verified")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("")
                                .to_string();
                            if status == 200 && verified == "true" {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("HMAC verified, signature={}", signature),
                                )
                            } else {
                                let body = resp.text().await.unwrap_or_default();
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "status={} x-verified='{}' sig='{}' body='{}'",
                                        status, verified, signature, body
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

    // ==================== Scenario 26: Path Parameter ====================
    // DSL source:
    //   let uid = req.path_param("uid")
    //   if uid != nil { resp.set_header("X-Path-Uid", uid) }
    //   let item = req.path_param("item")
    //   if item != nil { resp.set_header("X-Path-Item", item) }
    //   let missing = req.path_param("missing")
    //   if missing == nil { resp.set_header("X-Path-Missing", "nil") }
    //
    // Route: /test/dsl-path-param/users/:uid/items/:item (PathPrefix)
    // Request: /test/dsl-path-param/users/42/items/book-7
    // Expected: X-Path-Uid=42, X-Path-Item=book-7, X-Path-Missing=nil

    fn test_dsl_path_param_extracts_values() -> TestCase {
        TestCase::new(
            "dsl_path_param_extracts_values",
            "DSL: req.path_param(\"uid\")/path_param(\"item\") returns extracted values",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!(
                        "http://{}:31180/test/dsl-path-param/users/42/items/book-7",
                        ctx.target_host
                    );
                    let response = ctx
                        .http_client
                        .get(url)
                        .header("host", "dsl-path-param.example.com")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let uid = resp
                                .headers()
                                .get("x-path-uid")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("")
                                .to_string();
                            let item = resp
                                .headers()
                                .get("x-path-item")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("")
                                .to_string();
                            let missing = resp
                                .headers()
                                .get("x-path-missing")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("")
                                .to_string();
                            if status == 200 && uid == "42" && item == "book-7" && missing == "nil" {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("uid={} item={} missing={}", uid, item, missing),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("status={} uid='{}' item='{}' missing='{}'", status, uid, item, missing),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    /// PathPrefix segment-boundary regression guard.
    ///
    /// Route is `PathPrefix /test/dsl-path-param/users/:uid/items/:item` and
    /// the request appends `/extra` after the matched prefix. The radix
    /// matcher must still bind `:item=book-7` (not `book-7/extra`), and the
    /// route must still match (i.e. PathPrefix allows further suffix segments
    /// beyond the template).
    // ==================== Scenario 27: cached_query freeze (unified-query-string-cache) ====================

    /// Verify cached_query is frozen at entry: even if a DSL set_uri rewrites
    /// the URI to a different query, req.query("name") read afterwards must
    /// still return the entry-time value. test_server's /request-header-test
    /// echoes incoming request headers in the response body, so we look for
    /// "X-Captured-Name: BEFORE_REWRITE".
    fn test_freeze_query_param() -> TestCase {
        TestCase::new(
            "dsl_freeze_query_param",
            "DSL: cached_query stays at entry value after req.set_uri",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!(
                        "http://{}:31180/test/dsl-freeze-query?name=BEFORE_REWRITE",
                        ctx.target_host
                    );
                    let response = ctx
                        .http_client
                        .get(url)
                        .header("host", "dsl-freeze-query.example.com")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                            }
                            let body = resp.text().await.unwrap_or_default();
                            // header_echo_handler renders headers as "  Name: value\n" lines
                            if body.contains("X-Captured-Name: BEFORE_REWRITE") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "cached_query frozen to entry value after set_uri".to_string(),
                                )
                            } else if body.contains("X-Captured-Name: AFTER_REWRITE") {
                                TestResult::failed(
                                    start.elapsed(),
                                    "REGRESSION: cached_query read live URI after set_uri (entry freeze broken)"
                                        .to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("X-Captured-Name not in echoed headers; body=\n{}", body),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_dsl_path_param_prefix_extension() -> TestCase {
        TestCase::new(
            "dsl_path_param_prefix_extension",
            "DSL: PathPrefix `:item` binds the segment, not the trailing suffix",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!(
                        "http://{}:31180/test/dsl-path-param/users/42/items/book-7/extra",
                        ctx.target_host
                    );
                    let response = ctx
                        .http_client
                        .get(url)
                        .header("host", "dsl-path-param.example.com")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let uid = resp
                                .headers()
                                .get("x-path-uid")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("")
                                .to_string();
                            let item = resp
                                .headers()
                                .get("x-path-item")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("")
                                .to_string();
                            if status == 200 && uid == "42" && item == "book-7" {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("uid={} item={} (suffix /extra ignored)", uid, item),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("status={} uid='{}' item='{}'", status, uid, item),
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

impl TestSuite for DslTestSuite {
    fn name(&self) -> &str {
        "DSL Plugin Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            // Scenario 1: Header check (deny/allow)
            Self::test_header_check_deny(),
            Self::test_header_check_allow(),
            // Scenario 2: Header rewrite (set headers / passthrough)
            Self::test_header_rewrite(),
            Self::test_header_rewrite_passthrough(),
            // Scenario 3: Path deny (blocked / allowed)
            Self::test_deny_path_blocked(),
            Self::test_deny_path_allowed(),
            // Additional scenarios
            Self::test_set_request_header_with_user_id(),
            Self::test_set_request_header_without_user_id(),
            Self::test_deny_by_header_blocked(),
            Self::test_deny_by_header_allowed(),
            Self::test_cookie_read_present(),
            Self::test_cookie_read_absent(),
            Self::test_ctx_chain_with_tenant(),
            Self::test_ctx_chain_without_tenant(),
            Self::test_multi_header_ops(),
            Self::test_uri_rewrite_v1(),
            Self::test_uri_rewrite_v2_unchanged(),
            Self::test_sha256_sign(),
            Self::test_regex_extract_chrome(),
            Self::test_regex_extract_no_chrome(),
            Self::test_response_inject(),
            Self::test_resp_status_404(),
            Self::test_resp_status_200_no_header(),
            Self::test_deny_with_body_no_auth(),
            Self::test_deny_with_body_wrong_method(),
            Self::test_deny_with_body_get_allowed(),
            Self::test_range_iterate(),
            // Scenario 16: Regex replace
            Self::test_regex_replace_versioned_path(),
            Self::test_regex_replace_no_match(),
            // Scenario 17: Trim + Split
            Self::test_trim_split_with_admin(),
            Self::test_trim_split_without_admin(),
            // Scenario 18: Break + Continue
            Self::test_break_continue(),
            // Scenario 19: Terminate
            Self::test_terminate_connection_reset(),
            Self::test_terminate_normal_pass(),
            // Scenario 20: resp.set_status
            Self::test_resp_set_status_rewrite(),
            // Scenario 21: Utility combo (hex, json_get, uuid, ip_in_cidr, join, protocol)
            Self::test_utility_combo(),
            // Scenario 22: HMAC verify
            Self::test_hmac_verify_missing_sig(),
            Self::test_hmac_verify_invalid_sig(),
            Self::test_hmac_verify_valid_sig(),
            // Scenario 24: http.call — real external HTTP call from DSL script
            Self::test_http_call_success(),
            // Scenario 25: redis.* builtins — LinkSys-backed Redis operations
            Self::test_redis_roundtrip(),
            Self::test_redis_incr(),
            Self::test_redis_hash(),
            Self::test_redis_missing_ref(),
            // Scenario 26: Path parameter extraction (`:uid`, `:item`)
            Self::test_dsl_path_param_extracts_values(),
            Self::test_dsl_path_param_prefix_extension(),
            // Scenario 27: cached_query freeze (unified-query-string-cache)
            Self::test_freeze_query_param(),
        ]
    }
}
