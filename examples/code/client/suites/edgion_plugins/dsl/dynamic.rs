// DSL Plugin Dynamic Configuration Test Suite
//
// Tests dynamic update of DSL plugin rules.
// Verifies that when the EdgionPlugins YAML is updated (hot-reloaded),
// the gateway enforces the new DSL logic.
//
// Initial config: X-Version "v1" → allow (200), anything else → deny (403)
// Updated config: X-Version "v2" → allow (200), anything else → deny (403)
//
// Config files:
//   conf/EdgionPlugins/Dsl/DynamicTest/initial/EdgionPlugins_dsl-dynamic.yaml
//   conf/EdgionPlugins/Dsl/DynamicTest/initial/HTTPRoute_dsl-dynamic.yaml
//   conf/EdgionPlugins/Dsl/DynamicTest/updates/EdgionPlugins_dsl-dynamic.yaml

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

const HOST: &str = "dsl-dynamic.example.com";

// =============================================================================
// Initial phase: v1 allowed, v2 denied
// =============================================================================

pub struct DslDynamicInitialTestSuite;

impl DslDynamicInitialTestSuite {
    pub fn new() -> Self {
        Self
    }

    /// Initial: X-Version: v1 → 200 (allowed)
    fn test_initial_v1_allowed() -> TestCase {
        TestCase::new(
            "dsl_dynamic_initial_v1_allowed",
            "[INITIAL] DSL dynamic: X-Version=v1 should be allowed (200)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-dynamic/api", ctx.target_host);
                    let response = ctx
                        .http_client
                        .get(url)
                        .header("host", HOST)
                        .header("X-Version", "v1")
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

    /// Initial: X-Version: v2 → 403 (denied)
    fn test_initial_v2_denied() -> TestCase {
        TestCase::new(
            "dsl_dynamic_initial_v2_denied",
            "[INITIAL] DSL dynamic: X-Version=v2 should be denied (403)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-dynamic/api", ctx.target_host);
                    let response = ctx
                        .http_client
                        .get(url)
                        .header("host", HOST)
                        .header("X-Version", "v2")
                        .send()
                        .await;

                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status == 403 {
                                TestResult::passed(start.elapsed())
                            } else {
                                let body = resp.text().await.unwrap_or_default();
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
}

impl Default for DslDynamicInitialTestSuite {
    fn default() -> Self {
        Self::new()
    }
}

impl TestSuite for DslDynamicInitialTestSuite {
    fn name(&self) -> &str {
        "DSL Dynamic Tests - Initial Phase"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_initial_v1_allowed(), Self::test_initial_v2_denied()]
    }
}

// =============================================================================
// Update phase: v1 denied, v2 allowed
// =============================================================================

pub struct DslDynamicUpdateTestSuite;

impl DslDynamicUpdateTestSuite {
    pub fn new() -> Self {
        Self
    }

    /// Updated: X-Version: v1 → 403 (denied after update)
    fn test_updated_v1_denied() -> TestCase {
        TestCase::new(
            "dsl_dynamic_updated_v1_denied",
            "[AFTER UPDATE] DSL dynamic: X-Version=v1 should be denied (403)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-dynamic/api", ctx.target_host);
                    let response = ctx
                        .http_client
                        .get(url)
                        .header("host", HOST)
                        .header("X-Version", "v1")
                        .send()
                        .await;

                    match response {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status == 403 {
                                TestResult::passed(start.elapsed())
                            } else {
                                let body = resp.text().await.unwrap_or_default();
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

    /// Updated: X-Version: v2 → 200 (allowed after update)
    fn test_updated_v2_allowed() -> TestCase {
        TestCase::new(
            "dsl_dynamic_updated_v2_allowed",
            "[AFTER UPDATE] DSL dynamic: X-Version=v2 should be allowed (200)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("http://{}:31180/test/dsl-dynamic/api", ctx.target_host);
                    let response = ctx
                        .http_client
                        .get(url)
                        .header("host", HOST)
                        .header("X-Version", "v2")
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

impl Default for DslDynamicUpdateTestSuite {
    fn default() -> Self {
        Self::new()
    }
}

impl TestSuite for DslDynamicUpdateTestSuite {
    fn name(&self) -> &str {
        "DSL Dynamic Tests - Update Phase"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_updated_v1_denied(), Self::test_updated_v2_allowed()]
    }
}
