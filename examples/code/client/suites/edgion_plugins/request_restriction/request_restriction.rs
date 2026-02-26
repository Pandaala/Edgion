// RequestRestriction Plugin Test Suite
//
// ：
// - Header  (User-Agent Bot )
// - Path  ()
// - Method  ( API)
// - Header  ( Token)
// - 

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

pub struct RequestRestrictionTestSuite;

impl RequestRestrictionTestSuite {
    // ==================== 1. Header  ====================
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

    // ==================== 2. Method  ====================
    fn test_method_allow_get() -> TestCase {
        TestCase::new(
            "method_allow_get",
            "Method : GET  (200)",
            |ctx: TestContext| {
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
            },
        )
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

    // ==================== 3. Header  ====================
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
                    //  X-Auth-Token

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

    // ==================== 4.  ====================
    fn test_combined_normal() -> TestCase {
        TestCase::new(
            "combined_normal",
            ":  (200)",
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
                                TestResult::failed(start.elapsed(), format!("Expected status 200, got {}", status))
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
            ": Bot UA  (403)",
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
                                TestResult::failed(start.elapsed(), format!("Expected status 403, got {}", status))
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
            ": Admin  (403)",
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
            // Header 
            Self::test_header_deny_blocks_bot(),
            Self::test_header_deny_allows_normal(),
            // Method 
            Self::test_method_allow_get(),
            Self::test_method_allow_blocks_post(),
            // Header 
            Self::test_header_required_with_token(),
            Self::test_header_required_missing(),
            // 
            Self::test_combined_normal(),
            Self::test_combined_bot_blocked(),
            Self::test_combined_admin_blocked(),
        ]
    }
}
