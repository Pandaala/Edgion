// CtxSet Plugin Test Suite
//
// Verifies CtxSet plugin scenarios via the Access Log Store:
// - Set ctx fields from request headers, with optional default, transform, and mapping
// - Read back ctx fields from Access Log stage_logs to confirm they were written correctly
//
// Required config: conf/EdgionPlugins/CtxSet/01_EdgionPlugins_ctx-setter.yaml

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

pub struct CtxSetTestSuite;

impl CtxSetTestSuite {
    // ==================== 1. Set ctx from header ====================
    fn test_ctx_from_header() -> TestCase {
        TestCase::new("ctx_set_from_header", "CtxSet:  Header  ctx ", |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();
                let client = &ctx.http_client;
                let url = format!("http://{}:31180/test/ctx-setter/api", ctx.target_host);

                let trace_id = format!("test-ctx-header-{}", uuid::Uuid::new_v4());

                let response = client
                    .get(url)
                    .header("host", "ctx-setter.example.com")
                    .header("X-Tenant-Id", "acme-corp")
                    .header(
                        "traceparent",
                        crate::access_log_client::traceparent_for_marker(&trace_id),
                    )
                    .header("access_log", "test_store")
                    .send()
                    .await;

                match response {
                    Ok(resp) => {
                        let status = resp.status().as_u16();
                        if status != 200 {
                            return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                        }

                        // Fetch from access log store.
                        let al_client = ctx.access_log_client();
                        let entry = match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                            Ok(e) => e,
                            Err(e) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Failed to fetch access log: {}", e),
                                )
                            }
                        };

                        let access_log = entry.data.to_string();

                        // Verify tenant_id in ctx.
                        if access_log.contains(r#""tenant_id":"acme-corp""#) {
                            TestResult::passed_with_message(
                                start.elapsed(),
                                "ctx.tenant_id correctly set from header".to_string(),
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
        })
    }

    // ==================== 2. Default value when header absent ====================
    fn test_ctx_default_value() -> TestCase {
        TestCase::new("ctx_set_default_value", "CtxSet:  Header ", |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();
                let client = &ctx.http_client;
                let url = format!("http://{}:31180/test/ctx-setter/api", ctx.target_host);

                let trace_id = format!("test-ctx-default-{}", uuid::Uuid::new_v4());

                // No X-Tenant-Id header sent; expect the configured default value.
                let response = client
                    .get(url)
                    .header("host", "ctx-setter.example.com")
                    .header(
                        "traceparent",
                        crate::access_log_client::traceparent_for_marker(&trace_id),
                    )
                    .header("access_log", "test_store")
                    .send()
                    .await;

                match response {
                    Ok(resp) => {
                        let status = resp.status().as_u16();
                        if status != 200 {
                            return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                        }

                        let al_client = ctx.access_log_client();
                        let entry = match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                            Ok(e) => e,
                            Err(e) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Failed to fetch access log: {}", e),
                                )
                            }
                        };

                        let access_log = entry.data.to_string();

                        // Verify default tenant_id in ctx.
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
        })
    }

    // ==================== 3. Case transform (method -> lower) ====================
    fn test_ctx_transform_case() -> TestCase {
        TestCase::new(
            "ctx_set_transform_case",
            "CtxSet:  (method -> lower)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("http://{}:31180/test/ctx-setter/api", ctx.target_host);

                    let trace_id = format!("test-ctx-transform-{}", uuid::Uuid::new_v4());

                    let response = client
                        .get(url)
                        .header("host", "ctx-setter.example.com")
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker(&trace_id),
                        )
                        .header("access_log", "test_store")
                        .send()
                        .await;

                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                            }

                            let al_client = ctx.access_log_client();
                            let entry = match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                                Ok(e) => e,
                                Err(e) => {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!("Failed to fetch access log: {}", e),
                                    )
                                }
                            };

                            let access_log = entry.data.to_string();

                            // Verify method transformed to lowercase in ctx.
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

    // ==================== 4. Value mapping (premium -> tier_1) ====================
    fn test_ctx_mapping() -> TestCase {
        TestCase::new("ctx_set_mapping", "CtxSet:  (premium -> tier_1)", |ctx: TestContext| {
            Box::pin(async move {
                let start = Instant::now();
                let client = &ctx.http_client;
                let url = format!("http://{}:31180/test/ctx-setter/api", ctx.target_host);

                let trace_id = format!("test-ctx-mapping-{}", uuid::Uuid::new_v4());

                let response = client
                    .get(url)
                    .header("host", "ctx-setter.example.com")
                    .header("X-Plan", "premium")
                    .header(
                        "traceparent",
                        crate::access_log_client::traceparent_for_marker(&trace_id),
                    )
                    .header("access_log", "test_store")
                    .send()
                    .await;

                match response {
                    Ok(resp) => {
                        let status = resp.status().as_u16();
                        if status != 200 {
                            return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                        }

                        let al_client = ctx.access_log_client();
                        let entry = match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                            Ok(e) => e,
                            Err(e) => {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Failed to fetch access log: {}", e),
                                )
                            }
                        };

                        let access_log = entry.data.to_string();

                        // Verify premium maps to tier_1 in ctx.
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
        })
    }

    // ==================== 5. Mapping default fallback (unknown -> tier_3) ====================
    fn test_ctx_mapping_default() -> TestCase {
        TestCase::new(
            "ctx_set_mapping_default",
            "CtxSet:  (unknown -> tier_3)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("http://{}:31180/test/ctx-setter/api", ctx.target_host);

                    let trace_id = format!("test-ctx-map-def-{}", uuid::Uuid::new_v4());

                    let response = client
                        .get(url)
                        .header("host", "ctx-setter.example.com")
                        .header("X-Plan", "unknown-plan")
                        .header(
                            "traceparent",
                            crate::access_log_client::traceparent_for_marker(&trace_id),
                        )
                        .header("access_log", "test_store")
                        .send()
                        .await;

                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status != 200 {
                                return TestResult::failed(start.elapsed(), format!("Expected 200, got {}", status));
                            }

                            let al_client = ctx.access_log_client();
                            let entry = match al_client.get_access_log_with_retry(&trace_id, 10, 200).await {
                                Ok(e) => e,
                                Err(e) => {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!("Failed to fetch access log: {}", e),
                                    )
                                }
                            };

                            let access_log = entry.data.to_string();

                            // Verify mapping.default applied: unknown plan falls back to tier_3.
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
