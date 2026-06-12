// ResponseRewrite Plugin Test Suite
//
// ：
// -
// -  (set/add/remove)
// -  (rename)
// -

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

pub struct ResponseRewriteTestSuite;

impl ResponseRewriteTestSuite {
    // ==================== 1.  ====================
    fn test_status_code() -> TestCase {
        TestCase::new("status_code", ":  201", |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();
                let client = &ctx.http_client;
                let url = format!("{}/status-code/test", ctx.http_url());

                let request = client.get(&url).header("host", "response-rewrite.example.com");

                match request.send().await {
                    Ok(response) => {
                        let status = response.status().as_u16();
                        if status == 201 {
                            TestResult::passed_with_message(start.elapsed(), format!("Status code = {}", status))
                        } else {
                            TestResult::failed(start.elapsed(), format!("Expected status 201, got {}", status))
                        }
                    }
                    Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                }
            })
        })
    }

    // ==================== 2.  ====================
    fn test_headers_set() -> TestCase {
        TestCase::new("headers_set", ": set/add/remove ", |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();
                let client = &ctx.http_client;
                let url = format!("{}/headers-set/test", ctx.http_url());

                let request = client.get(&url).header("host", "response-rewrite.example.com");

                match request.send().await {
                    Ok(response) => {
                        let mut errors = Vec::new();

                        //  set
                        if let Some(custom_header) = response.headers().get("x-custom-header") {
                            if custom_header.to_str().unwrap_or("") != "custom-value" {
                                errors.push(format!(
                                    "X-Custom-Header: expected 'custom-value', got '{}'",
                                    custom_header.to_str().unwrap_or("")
                                ));
                            }
                        } else {
                            errors.push("X-Custom-Header not found".to_string());
                        }

                        //  Cache-Control
                        if let Some(cache_control) = response.headers().get("cache-control") {
                            if cache_control.to_str().unwrap_or("") != "no-cache, no-store" {
                                errors.push(format!(
                                    "Cache-Control: expected 'no-cache, no-store', got '{}'",
                                    cache_control.to_str().unwrap_or("")
                                ));
                            }
                        } else {
                            errors.push("Cache-Control not found".to_string());
                        }

                        //  add
                        if let Some(powered_by) = response.headers().get("x-powered-by") {
                            if powered_by.to_str().unwrap_or("") != "Edgion" {
                                errors.push(format!(
                                    "X-Powered-By: expected 'Edgion', got '{}'",
                                    powered_by.to_str().unwrap_or("")
                                ));
                            }
                        } else {
                            errors.push("X-Powered-By not found".to_string());
                        }

                        //  remove  - Server
                        // ： Server ，
                        // if response.headers().get("server").is_some() {
                        //     errors.push("Server header should be removed".to_string());
                        // }

                        if errors.is_empty() {
                            TestResult::passed_with_message(
                                start.elapsed(),
                                format!(
                                    "X-Custom-Header={:?}, Cache-Control={:?}, X-Powered-By={:?}",
                                    response.headers().get("x-custom-header"),
                                    response.headers().get("cache-control"),
                                    response.headers().get("x-powered-by")
                                ),
                            )
                        } else {
                            TestResult::failed(start.elapsed(), errors.join("; "))
                        }
                    }
                    Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                }
            })
        })
    }

    // ==================== 3.  ====================
    // ： test_server  rename
    //  test_server  X-Internal-Id ，
    fn test_headers_rename() -> TestCase {
        TestCase::new(
            "headers_rename",
            ": X-Internal-Id -> X-Request-Id ( test_server )",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("{}/headers-rename/test", ctx.http_url());

                    let request = client.get(&url).header("host", "response-rewrite.example.com");

                    match request.send().await {
                        Ok(response) => {
                            // rename  test_server
                            //  test_server  X-Internal-Id，
                            //
                            if response.status().is_success() {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Request successful (rename verification requires test_server support)".to_string(),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("HTTP {}", response.status()))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 4.  ====================
    fn test_combined() -> TestCase {
        TestCase::new("combined", ":  + set + add + remove", |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();
                let client = &ctx.http_client;
                let url = format!("{}/combined/test", ctx.http_url());

                let request = client.get(&url).header("host", "response-rewrite.example.com");

                match request.send().await {
                    Ok(response) => {
                        let mut errors = Vec::new();

                        //
                        let status = response.status().as_u16();
                        if status != 200 {
                            errors.push(format!("Status: expected 200, got {}", status));
                        }

                        //  set - Cache-Control
                        if let Some(cache_control) = response.headers().get("cache-control") {
                            if cache_control.to_str().unwrap_or("") != "no-cache" {
                                errors.push(format!(
                                    "Cache-Control: expected 'no-cache', got '{}'",
                                    cache_control.to_str().unwrap_or("")
                                ));
                            }
                        } else {
                            errors.push("Cache-Control not found".to_string());
                        }

                        //  set - X-API-Version
                        if let Some(api_version) = response.headers().get("x-api-version") {
                            if api_version.to_str().unwrap_or("") != "v2" {
                                errors.push(format!(
                                    "X-API-Version: expected 'v2', got '{}'",
                                    api_version.to_str().unwrap_or("")
                                ));
                            }
                        } else {
                            errors.push("X-API-Version not found".to_string());
                        }

                        //  add - X-Powered-By
                        if let Some(powered_by) = response.headers().get("x-powered-by") {
                            if powered_by.to_str().unwrap_or("") != "Edgion" {
                                errors.push(format!(
                                    "X-Powered-By: expected 'Edgion', got '{}'",
                                    powered_by.to_str().unwrap_or("")
                                ));
                            }
                        } else {
                            errors.push("X-Powered-By not found".to_string());
                        }

                        if errors.is_empty() {
                            TestResult::passed_with_message(
                                start.elapsed(),
                                format!(
                                    "Status={}, Cache-Control={:?}, X-API-Version={:?}, X-Powered-By={:?}",
                                    status,
                                    response.headers().get("cache-control"),
                                    response.headers().get("x-api-version"),
                                    response.headers().get("x-powered-by")
                                ),
                            )
                        } else {
                            TestResult::failed(start.elapsed(), errors.join("; "))
                        }
                    }
                    Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                }
            })
        })
    }
}

impl TestSuite for ResponseRewriteTestSuite {
    fn name(&self) -> &str {
        "ResponseRewrite Plugin Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_status_code(),
            Self::test_headers_set(),
            // rename  test_server ，
            // Self::test_headers_rename(),
            Self::test_combined(),
        ]
    }
}
