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
                    let url = "http://127.0.0.1:31180/test/dsl-header-check/api";

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
                    let url = "http://127.0.0.1:31180/test/dsl-header-check/api";

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
                    let url = "http://127.0.0.1:31180/test/dsl-header-rewrite/api";

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
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200, got {}", status),
                                );
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
                    let url = "http://127.0.0.1:31180/test/dsl-header-rewrite/api";

                    let response = client
                        .get(url)
                        .header("host", "dsl-header-rewrite.example.com")
                        .send()
                        .await;

                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected 200, got {}", status),
                                );
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
                    let url = "http://127.0.0.1:31180/test/dsl-deny-path/admin/secret";

                    let response = client
                        .get(url)
                        .header("host", "dsl-deny-path.example.com")
                        .send()
                        .await;

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
                    let url = "http://127.0.0.1:31180/test/dsl-deny-path/api/users";

                    let response = client
                        .get(url)
                        .header("host", "dsl-deny-path.example.com")
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
        ]
    }
}
