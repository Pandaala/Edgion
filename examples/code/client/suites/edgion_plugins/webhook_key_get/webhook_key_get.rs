// WebhookKeyGet Integration Test Suite
//
// Tests the KeyGet::Webhook variant through the CtxSet plugin.
// The webhook calls test_server's /webhook/resolve endpoint which returns
// resolved values in response headers, cookies, and JSON body.
//
// Test strategy:
// - Send requests through Gateway with X-Tenant-Id header
// - Webhook service (test_server) resolves keys based on tenant
// - Verify ctx variables are correctly set via Access Log Store
//
// Config reference: conf/EdgionPlugins/WebhookKeyGet/

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

pub struct WebhookKeyGetTestSuite;

const TEST_HOST: &str = "webhook-keyget.example.com";

impl WebhookKeyGetTestSuite {
    // ==================== 1. Webhook JSON body extraction ====================
    fn test_webhook_body_json_extract() -> TestCase {
        TestCase::new(
            "webhook_body_json_extract",
            "Webhook: resolve user_id from JSON body path",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("http://{}:31180/test/webhook-keyget/api", ctx.target_host);

                    let trace_id = format!("test-wh-body-json-{}", uuid::Uuid::new_v4());

                    let response = client
                        .get(url)
                        .header("host", TEST_HOST)
                        .header("X-Tenant-Id", "acme-corp")
                        .header("x-trace-id", &trace_id)
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

                            // Verify webhook resolved user_id from JSON body path "data.user_id"
                            // test_server returns {"data":{"user_id":"uid-acme-corp",...}}
                            if access_log.contains(r#""webhook_user_id":"uid-acme-corp""#) {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "ctx.webhook_user_id correctly resolved from webhook JSON body".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected webhook_user_id=uid-acme-corp in ctx. Access-Log: {}",
                                        access_log
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

    // ==================== 2. Webhook response header extraction ====================
    fn test_webhook_header_extract() -> TestCase {
        TestCase::new(
            "webhook_header_extract",
            "Webhook: resolve key from response header",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("http://{}:31180/test/webhook-keyget/api", ctx.target_host);

                    let trace_id = format!("test-wh-header-{}", uuid::Uuid::new_v4());

                    let response = client
                        .get(url)
                        .header("host", TEST_HOST)
                        .header("X-Tenant-Id", "beta-inc")
                        .header("x-trace-id", &trace_id)
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

                            // Verify webhook resolved key from X-Resolved-Key response header
                            // test_server returns X-Resolved-Key: resolved-beta-inc
                            if access_log.contains(r#""webhook_resolved_key":"resolved-beta-inc""#) {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "ctx.webhook_resolved_key correctly resolved from webhook response header"
                                        .to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected webhook_resolved_key=resolved-beta-inc in ctx. Access-Log: {}",
                                        access_log
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

    // ==================== 3. Webhook body text extraction ====================
    fn test_webhook_body_text_extract() -> TestCase {
        TestCase::new(
            "webhook_body_text_extract",
            "Webhook: resolve key from plain text body",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("http://{}:31180/test/webhook-keyget/api", ctx.target_host);

                    let trace_id = format!("test-wh-bodytext-{}", uuid::Uuid::new_v4());

                    let response = client
                        .get(url)
                        .header("host", TEST_HOST)
                        .header("X-Tenant-Id", "gamma-llc")
                        .header("x-trace-id", &trace_id)
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

                            // Verify webhook resolved key from plain text body (trimmed)
                            // test_server returns "  body-key-gamma-llc  " which gets trimmed
                            if access_log.contains(r#""webhook_body_text":"body-key-gamma-llc""#) {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "ctx.webhook_body_text correctly resolved from webhook body text".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected webhook_body_text=body-key-gamma-llc in ctx. Access-Log: {}",
                                        access_log
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

    // ==================== 4. Local key_get still works alongside webhook ====================
    fn test_local_key_get_alongside_webhook() -> TestCase {
        TestCase::new(
            "local_key_get_with_webhook",
            "Webhook: local key_get (header) still works alongside webhook variants",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("http://{}:31180/test/webhook-keyget/api", ctx.target_host);

                    let trace_id = format!("test-wh-local-{}", uuid::Uuid::new_v4());

                    let response = client
                        .get(url)
                        .header("host", TEST_HOST)
                        .header("X-Tenant-Id", "delta-org")
                        .header("x-trace-id", &trace_id)
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

                            // Verify local key_get from header still works
                            if access_log.contains(r#""local_tenant":"delta-org""#) {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "ctx.local_tenant correctly set from header alongside webhook vars".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected local_tenant=delta-org in ctx. Access-Log: {}", access_log),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // ==================== 5. Webhook with default fallback ====================
    fn test_webhook_default_fallback() -> TestCase {
        TestCase::new(
            "webhook_default_fallback",
            "Webhook: uses default value when tenant header is missing",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("http://{}:31180/test/webhook-keyget/api", ctx.target_host);

                    let trace_id = format!("test-wh-default-{}", uuid::Uuid::new_v4());

                    // Send request WITHOUT X-Tenant-Id header
                    // Webhook will still resolve but with "unknown" tenant
                    let response = client
                        .get(url)
                        .header("host", TEST_HOST)
                        .header("x-trace-id", &trace_id)
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

                            // Webhook resolves with "unknown" tenant (default from test_server)
                            // So user_id should be "uid-unknown"
                            if access_log.contains(r#""webhook_user_id":"uid-unknown""#) {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "ctx.webhook_user_id correctly resolved with default tenant".to_string(),
                                )
                            } else if access_log.contains(r#""webhook_user_id":"fallback-user""#) {
                                // If webhook call failed, default value should be used
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "ctx.webhook_user_id uses fallback default value".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected webhook_user_id=uid-unknown or fallback-user. Access-Log: {}",
                                        access_log
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

    // ==================== 6. Webhook with missing tenant uses local default ====================
    fn test_local_default_without_header() -> TestCase {
        TestCase::new(
            "local_default_without_header",
            "Webhook: local ctx var uses default when header is missing",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let client = &ctx.http_client;
                    let url = format!("http://{}:31180/test/webhook-keyget/api", ctx.target_host);

                    let trace_id = format!("test-wh-noheader-{}", uuid::Uuid::new_v4());

                    // No X-Tenant-Id header — local_tenant should use default "no-tenant"
                    let response = client
                        .get(url)
                        .header("host", TEST_HOST)
                        .header("x-trace-id", &trace_id)
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

                            if access_log.contains(r#""local_tenant":"no-tenant""#) {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "ctx.local_tenant correctly uses default 'no-tenant'".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected local_tenant=no-tenant in ctx. Access-Log: {}", access_log),
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

impl TestSuite for WebhookKeyGetTestSuite {
    fn name(&self) -> &str {
        "WebhookKeyGet Plugin Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_webhook_body_json_extract(),
            Self::test_webhook_header_extract(),
            Self::test_webhook_body_text_extract(),
            Self::test_local_key_get_alongside_webhook(),
            Self::test_webhook_default_fallback(),
            Self::test_local_default_without_header(),
        ]
    }
}
