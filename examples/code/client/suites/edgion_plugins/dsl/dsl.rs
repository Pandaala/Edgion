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

    // ==================== 场景 4: Set Request Header ====================

    fn test_set_request_header_with_user_id() -> TestCase {
        TestCase::new(
            "dsl_set_request_header_with_user_id",
            "DSL: X-User-Id: 42 → X-Processed-User: uid-42 echoed by test_server",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-set-request-header/headers", ctx.target_host);
                    let response = ctx.http_client
                        .get(url)
                        .header("host", "dsl-set-request-header.example.com")
                        .header("X-User-Id", "42")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let body = resp.text().await.unwrap_or_default();
                            if status == 200
                                && body.to_lowercase().contains("x-processed-user")
                                && body.contains("uid-42")
                            {
                                TestResult::passed_with_message(start.elapsed(), "X-Processed-User: uid-42 echoed".into())
                            } else {
                                TestResult::failed(start.elapsed(), format!("status={} body={}", status, body))
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
            "DSL: no X-User-Id → no X-Processed-User in echo",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-set-request-header/headers", ctx.target_host);
                    let response = ctx.http_client
                        .get(url)
                        .header("host", "dsl-set-request-header.example.com")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let body = resp.text().await.unwrap_or_default();
                            if status == 200 && !body.to_lowercase().contains("x-processed-user") {
                                TestResult::passed_with_message(start.elapsed(), "No X-Processed-User as expected".into())
                            } else {
                                TestResult::failed(start.elapsed(), format!("status={} body={}", status, body))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 场景 5: Deny by Header ====================

    fn test_deny_by_header_blocked() -> TestCase {
        TestCase::new(
            "dsl_deny_by_header_blocked",
            "DSL: X-Block: 1 → 403 blocked",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-deny-by-header/api", ctx.target_host);
                    let response = ctx.http_client
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
                                TestResult::failed(start.elapsed(), format!("Expected 403/blocked, got {}/{}", status, body))
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
                    let response = ctx.http_client
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

    // ==================== 场景 6: Cookie Read ====================

    fn test_cookie_read_present() -> TestCase {
        TestCase::new(
            "dsl_cookie_read_present",
            "DSL: session_id=abc123 cookie → X-Session: abc123",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-cookie-read/headers", ctx.target_host);
                    let response = ctx.http_client
                        .get(url)
                        .header("host", "dsl-cookie-read.example.com")
                        .header("Cookie", "session_id=abc123")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let body = resp.text().await.unwrap_or_default();
                            if status == 200 && body.to_lowercase().contains("x-session") && body.contains("abc123") {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(start.elapsed(), format!("status={} body={}", status, body))
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
            "DSL: no session_id cookie → X-Session: anonymous",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-cookie-read/headers", ctx.target_host);
                    let response = ctx.http_client
                        .get(url)
                        .header("host", "dsl-cookie-read.example.com")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let body = resp.text().await.unwrap_or_default();
                            if status == 200 && body.contains("anonymous") {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(start.elapsed(), format!("status={} body={}", status, body))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 场景 7: Ctx Chain ====================

    fn test_ctx_chain_with_tenant() -> TestCase {
        TestCase::new(
            "dsl_ctx_chain_with_tenant",
            "DSL ctx chain: X-Tenant → X-Internal-Tenant + X-Pipeline + no X-Tenant",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-ctx-chain/headers", ctx.target_host);
                    let response = ctx.http_client
                        .get(url)
                        .header("host", "dsl-ctx-chain.example.com")
                        .header("X-Tenant", "acme")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let body = resp.text().await.unwrap_or_default();
                            let has_internal = body.to_lowercase().contains("x-internal-tenant") && body.contains("acme");
                            let has_pipeline = body.to_lowercase().contains("x-pipeline") && body.contains("dsl-ok");
                            let no_tenant = !body.contains("X-Tenant: acme") && !body.contains("x-tenant: acme");
                            if status == 200 && has_internal && has_pipeline && no_tenant {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(start.elapsed(), format!(
                                    "status={} internal={} pipeline={} no_tenant={} body={}",
                                    status, has_internal, has_pipeline, no_tenant, body
                                ))
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
            "DSL ctx chain: no X-Tenant → no X-Internal-Tenant, no X-Pipeline",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-ctx-chain/headers", ctx.target_host);
                    let response = ctx.http_client
                        .get(url)
                        .header("host", "dsl-ctx-chain.example.com")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let body = resp.text().await.unwrap_or_default();
                            let no_internal = !body.to_lowercase().contains("x-internal-tenant");
                            let no_pipeline = !body.to_lowercase().contains("x-pipeline");
                            if status == 200 && no_internal && no_pipeline {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(start.elapsed(), format!("status={} body={}", status, body))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 场景 8: Multi Header Ops ====================

    fn test_multi_header_ops() -> TestCase {
        TestCase::new(
            "dsl_multi_header_ops",
            "DSL: set/append/remove/set_host all applied",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-multi-header-ops/headers", ctx.target_host);
                    let response = ctx.http_client
                        .get(url)
                        .header("host", "dsl-multi-header-ops.example.com")
                        .header("User-Agent", "test-runner/1.0")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let body = resp.text().await.unwrap_or_default();
                            let has_x_set = body.to_lowercase().contains("x-set") && body.contains("value1");
                            let has_first = body.contains("first");
                            let has_second = body.contains("second");
                            let no_ua = !body.to_lowercase().contains("user-agent: test-runner");
                            if status == 200 && has_x_set && has_first && has_second && no_ua {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(start.elapsed(), format!(
                                    "status={} x_set={} first={} second={} no_ua={} body={}",
                                    status, has_x_set, has_first, has_second, no_ua, body
                                ))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 场景 9: URI Rewrite ====================

    fn test_uri_rewrite_v1() -> TestCase {
        TestCase::new(
            "dsl_uri_rewrite_v1",
            "DSL: GET /v1/users → test_server receives /api/users",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-uri-rewrite/v1/users", ctx.target_host);
                    let response = ctx.http_client
                        .get(url)
                        .header("host", "dsl-uri-rewrite.example.com")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let body = resp.text().await.unwrap_or_default();
                            if status == 200 && body.contains("/api/users") {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(start.elapsed(), format!("status={} body={}", status, body))
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
            "DSL: GET /v2/items → path NOT rewritten",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-uri-rewrite/v2/items", ctx.target_host);
                    let response = ctx.http_client
                        .get(url)
                        .header("host", "dsl-uri-rewrite.example.com")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let body = resp.text().await.unwrap_or_default();
                            if status == 200 && !body.contains("/api/items") {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(start.elapsed(), format!("status={} body={}", status, body))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 场景 10: SHA256 Sign ====================

    fn test_sha256_sign() -> TestCase {
        TestCase::new(
            "dsl_sha256_sign",
            "DSL: X-Token: hello → X-Token-Hash: sha256(hello)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-sha256-sign/headers", ctx.target_host);
                    let response = ctx.http_client
                        .get(url)
                        .header("host", "dsl-sha256-sign.example.com")
                        .header("X-Token", "hello")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let body = resp.text().await.unwrap_or_default();
                            let expected = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
                            if status == 200 && body.contains(expected) {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(start.elapsed(), format!("status={} body={}", status, body))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 场景 11: Regex Extract ====================

    fn test_regex_extract_chrome() -> TestCase {
        TestCase::new(
            "dsl_regex_extract_chrome",
            "DSL: User-Agent Chrome/120 → X-Chrome-Ver: Chrome/120",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-regex-extract/headers", ctx.target_host);
                    let response = ctx.http_client
                        .get(url)
                        .header("host", "dsl-regex-extract.example.com")
                        .header("User-Agent", "Mozilla/5.0 Chrome/120.0 Safari/537.36")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let body = resp.text().await.unwrap_or_default();
                            if status == 200 && body.to_lowercase().contains("x-chrome-ver") && body.contains("Chrome/120") {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(start.elapsed(), format!("status={} body={}", status, body))
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
            "DSL: curl User-Agent → no X-Chrome-Ver",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-regex-extract/headers", ctx.target_host);
                    let response = ctx.http_client
                        .get(url)
                        .header("host", "dsl-regex-extract.example.com")
                        .header("User-Agent", "curl/7.0")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let body = resp.text().await.unwrap_or_default();
                            if status == 200 && !body.to_lowercase().contains("x-chrome-ver") {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(start.elapsed(), format!("status={} body={}", status, body))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 场景 12: Response Inject ====================

    fn test_response_inject() -> TestCase {
        TestCase::new(
            "dsl_response_inject",
            "DSL upstreamResponseFilter: CORS headers injected in response",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-response-inject/health", ctx.target_host);
                    let response = ctx.http_client
                        .get(url)
                        .header("host", "dsl-response-inject.example.com")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let acao = resp.headers().get("access-control-allow-origin")
                                .and_then(|v| v.to_str().ok()).unwrap_or("");
                            let powered_by = resp.headers().get("x-powered-by")
                                .and_then(|v| v.to_str().ok()).unwrap_or("");
                            let vary = resp.headers().get("vary")
                                .and_then(|v| v.to_str().ok()).unwrap_or("");
                            if status == 200 && acao == "*" && powered_by == "Edgion-DSL" && vary.contains("Origin") {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(start.elapsed(), format!(
                                    "status={} acao='{}' powered_by='{}' vary='{}'",
                                    status, acao, powered_by, vary
                                ))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 场景 13: Resp Status Conditional ====================

    fn test_resp_status_404() -> TestCase {
        TestCase::new(
            "dsl_resp_status_404",
            "DSL upstreamResponseFilter: 404 → X-Not-Found: true",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-resp-status-conditional/status/404", ctx.target_host);
                    let response = ctx.http_client
                        .get(url)
                        .header("host", "dsl-resp-status-conditional.example.com")
                        .send()
                        .await;
                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            let not_found = resp.headers().get("x-not-found")
                                .and_then(|v| v.to_str().ok()).unwrap_or("");
                            if status == 404 && not_found == "true" {
                                TestResult::passed(start.elapsed())
                            } else {
                                TestResult::failed(start.elapsed(), format!("status={} x-not-found='{}'", status, not_found))
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
                    let url = format!("http://{}:31180/test/dsl-resp-status-conditional/status/200", ctx.target_host);
                    let response = ctx.http_client
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
                                TestResult::failed(start.elapsed(), format!("status={} has_x_not_found={}", status, has_not_found))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 场景 14: Deny with Body ====================

    fn test_deny_with_body_no_auth() -> TestCase {
        TestCase::new(
            "dsl_deny_with_body_no_auth",
            "DSL: no Authorization → 401 with JSON error body",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-deny-with-body/api", ctx.target_host);
                    let response = ctx.http_client
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
                                TestResult::failed(start.elapsed(), format!("Expected 401/unauthorized, got {}/{}", status, body))
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
                    let response = ctx.http_client
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
                    let response = ctx.http_client
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
            // 新场景
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
        ]
    }
}
