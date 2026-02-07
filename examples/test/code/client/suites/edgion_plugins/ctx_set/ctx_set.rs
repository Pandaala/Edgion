// CtxSet Plugin Test Suite
//
// 测试策略：
// - 通过 DebugAccessLogToHeader 插件验证 ctx 变量被正确设置
// - DebugAccessLogToHeader 会在响应头中添加 X-Edgion-Ctx-{name} 格式的头
//
// 配置参考：conf/EdgionPlugins/CtxSet/01_EdgionPlugins_ctx-setter.yaml

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

pub struct CtxSetTestSuite;

impl CtxSetTestSuite {
    // ==================== 1. 从 Header 设置 ctx 变量测试 ====================
    fn test_ctx_from_header() -> TestCase {
        TestCase::new(
            "ctx_set_from_header",
            "CtxSet: 从 Header 设置 ctx 变量",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = "http://127.0.0.1:31180/test/ctx-setter/api";

                    let response = client
                        .get(url)
                        .header("host", "ctx-setter.example.com")
                        .header("X-Tenant-Id", "acme-corp")
                        .send()
                        .await;

                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                            }

                            // 检查 DebugAccessLogToHeader 设置的响应头
                            // ctx 变量通过 X-Edgion-Access-Log 头传递
                            let access_log = resp
                                .headers()
                                .get("X-Debug-Access-Log")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("");

                            // 验证 tenant_id 被设置
                            if access_log.contains(r#""tenant_id":"acme-corp""#) {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("ctx.tenant_id correctly set from header"),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("tenant_id not found in ctx. Access-Log: {}", access_log),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 2. 默认值测试 ====================
    fn test_ctx_default_value() -> TestCase {
        TestCase::new(
            "ctx_set_default_value",
            "CtxSet: 缺少 Header 时使用默认值",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = "http://127.0.0.1:31180/test/ctx-setter/api";

                    // 不发送 X-Tenant-Id header，应使用默认值
                    let response = client.get(url).header("host", "ctx-setter.example.com").send().await;

                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                            }

                            let access_log = resp
                                .headers()
                                .get("X-Debug-Access-Log")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("");

                            // 验证使用了默认值
                            if access_log.contains(r#""tenant_id":"default-tenant""#) {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "ctx.tenant_id correctly uses default value".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected default-tenant, Access-Log: {}", access_log),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 3. 大小写转换测试 ====================
    fn test_ctx_transform_case() -> TestCase {
        TestCase::new(
            "ctx_set_transform_case",
            "CtxSet: 大小写转换 (method -> lower)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = "http://127.0.0.1:31180/test/ctx-setter/api";

                    let response = client.get(url).header("host", "ctx-setter.example.com").send().await;

                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                            }

                            let access_log = resp
                                .headers()
                                .get("X-Debug-Access-Log")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("");

                            // 验证 method 被转换为小写
                            if access_log.contains(r#""method_lower":"get""#) {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "ctx.method_lower correctly transformed to lowercase".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected method_lower=get, Access-Log: {}", access_log),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 4. 值映射测试 ====================
    fn test_ctx_mapping() -> TestCase {
        TestCase::new(
            "ctx_set_mapping",
            "CtxSet: 值映射 (premium -> tier_1)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = "http://127.0.0.1:31180/test/ctx-setter/api";

                    let response = client
                        .get(url)
                        .header("host", "ctx-setter.example.com")
                        .header("X-Plan", "premium")
                        .send()
                        .await;

                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                            }

                            let access_log = resp
                                .headers()
                                .get("X-Debug-Access-Log")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("");

                            // 验证 premium 被映射为 tier_1
                            if access_log.contains(r#""tier":"tier_1""#) {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "ctx.tier correctly mapped: premium -> tier_1".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected tier=tier_1, Access-Log: {}", access_log),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 5. 映射默认值测试 ====================
    fn test_ctx_mapping_default() -> TestCase {
        TestCase::new(
            "ctx_set_mapping_default",
            "CtxSet: 映射默认值 (unknown -> tier_3)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = "http://127.0.0.1:31180/test/ctx-setter/api";

                    let response = client
                        .get(url)
                        .header("host", "ctx-setter.example.com")
                        .header("X-Plan", "unknown-plan")
                        .send()
                        .await;

                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                            }

                            let access_log = resp
                                .headers()
                                .get("X-Debug-Access-Log")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("");

                            // 验证未匹配时使用 mapping.default
                            if access_log.contains(r#""tier":"tier_3""#) {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "ctx.tier correctly uses mapping default: tier_3".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected tier=tier_3, Access-Log: {}", access_log),
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

impl TestSuite for CtxSetTestSuite {
    fn name(&self) -> &str {
        "CtxSet Plugin Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_ctx_from_header(),
            Self::test_ctx_default_value(),
            Self::test_ctx_transform_case(),
            Self::test_ctx_mapping(),
            Self::test_ctx_mapping_default(),
        ]
    }
}
