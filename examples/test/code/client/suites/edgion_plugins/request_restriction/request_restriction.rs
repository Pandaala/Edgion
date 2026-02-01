// RequestRestriction Plugin Test Suite
//
// 测试策略：
// - Header 拒绝列表测试 (User-Agent Bot 检测)
// - Path 允许列表测试 (路径白名单)
// - Method 允许列表测试 (只读 API)
// - Header 必须存在测试 (认证 Token)
// - 综合功能测试

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

pub struct RequestRestrictionTestSuite;

impl RequestRestrictionTestSuite {
    // ==================== 1. Header 拒绝列表测试 ====================
    fn test_header_deny_blocks_bot() -> TestCase {
        TestCase::new(
            "header_deny_blocks_bot",
            "Header 拒绝: Bot User-Agent 应被阻止 (403)",
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
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected status 403, got {}", status),
                                )
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
            "Header 拒绝: 正常 User-Agent 应通过 (200)",
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
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected status 200, got {}", status),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 2. Method 允许列表测试 ====================
    fn test_method_allow_get() -> TestCase {
        TestCase::new(
            "method_allow_get",
            "Method 允许: GET 请求应通过 (200)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/test/method-allow/api", ctx.http_url());

                    let request = client
                        .get(&url)
                        .header("host", "request-restriction.example.com");

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("GET passed with status {}", status),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected status 200, got {}", status),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_method_allow_blocks_post() -> TestCase {
        TestCase::new(
            "method_allow_blocks_post",
            "Method 允许: POST 请求应被阻止 (405)",
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
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected status 405, got {}", status),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 3. Header 必须存在测试 ====================
    fn test_header_required_with_token() -> TestCase {
        TestCase::new(
            "header_required_with_token",
            "Header 必须: 有效 Token 应通过 (200)",
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
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected status 200, got {}", status),
                                )
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
            "Header 必须: 缺少 Token 应被拒绝 (401)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/test/header-required/api", ctx.http_url());

                    let request = client
                        .get(&url)
                        .header("host", "request-restriction.example.com");
                    // 不发送 X-Auth-Token

                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 401 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Missing token rejected with status {}", status),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected status 401, got {}", status),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 4. 综合测试 ====================
    fn test_combined_normal() -> TestCase {
        TestCase::new(
            "combined_normal",
            "综合测试: 正常请求应通过 (200)",
            |ctx: TestContext| {
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
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected status 200, got {}", status),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_combined_bot_blocked() -> TestCase {
        TestCase::new(
            "combined_bot_blocked",
            "综合测试: Bot UA 应被阻止 (403)",
            |ctx: TestContext| {
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
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected status 403, got {}", status),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    fn test_combined_admin_blocked() -> TestCase {
        TestCase::new(
            "combined_admin_blocked",
            "综合测试: Admin 路径应被阻止 (403)",
            |ctx: TestContext| {
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
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected status 403, got {}", status),
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

impl TestSuite for RequestRestrictionTestSuite {
    fn name(&self) -> &str {
        "RequestRestriction Plugin Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            // Header 拒绝列表测试
            Self::test_header_deny_blocks_bot(),
            Self::test_header_deny_allows_normal(),
            // Method 允许列表测试
            Self::test_method_allow_get(),
            Self::test_method_allow_blocks_post(),
            // Header 必须存在测试
            Self::test_header_required_with_token(),
            Self::test_header_required_missing(),
            // 综合测试
            Self::test_combined_normal(),
            Self::test_combined_bot_blocked(),
            Self::test_combined_admin_blocked(),
        ]
    }
}
