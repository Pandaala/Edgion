// LinkSys Status Integration Test Suite
//
// Verifies the Accepted status condition on LinkSys resources.
//
// Required conf files (edgion-tests/integration/conf/LinkSys/Status/):
//   01_LinkSys_edgion-test_ls-status-redis-valid.yaml          — Redis with endpoints → Accepted=True
//   02_LinkSys_edgion-test_ls-status-redis-empty-endpoints.yaml — Redis with no endpoints → Accepted=False/Invalid
//
// Scenarios:
//   1. linksys_valid_redis_accepted       → Accepted=True
//   2. linksys_invalid_empty_endpoints_rejected → Accepted=False/Invalid, message contains "endpoints"

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

const NS: &str = "edgion-test";

pub struct LinkSysStatusTestSuite;

impl LinkSysStatusTestSuite {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LinkSysStatusTestSuite {
    fn default() -> Self {
        Self::new()
    }
}

/// Fetch status.conditions for a LinkSys from the configserver API.
/// Returns Ok(conditions_array) on success or Err(last_error_message) after 20 retries.
async fn fetch_conditions(ctx: &TestContext, name: &str) -> Result<Vec<serde_json::Value>, String> {
    let url = format!(
        "{}/configserver/LinkSys?namespace={}&name={}",
        ctx.admin_api_url(),
        NS,
        name
    );

    let mut last_err = format!("LinkSys '{}' not found", name);

    for _ in 0..20 {
        let mut req = ctx.http_client.get(&url);
        if let Some(ref token) = ctx.admin_token {
            req = req.header("Authorization", format!("Bearer {}", token));
        }

        match req.send().await {
            Ok(resp) if resp.status().is_success() => match resp.json::<serde_json::Value>().await {
                Ok(body) if body["success"].as_bool().unwrap_or(false) => {
                    if let Some(conditions) = body["data"]["status"]["conditions"].as_array() {
                        if !conditions.is_empty() {
                            return Ok(conditions.clone());
                        }
                        last_err = format!("LinkSys '{}' found but status.conditions is empty", name);
                    } else {
                        last_err = format!("LinkSys '{}' found but status.conditions not present", name);
                    }
                }
                Ok(body) => {
                    last_err = format!("API error: {}", body["error"].as_str().unwrap_or("unknown"));
                }
                Err(e) => {
                    last_err = format!("JSON parse error: {}", e);
                }
            },
            Ok(resp) => {
                last_err = format!("HTTP {}", resp.status());
            }
            Err(e) => {
                last_err = format!("Request error: {}", e);
            }
        }

        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    Err(last_err)
}

fn find_condition<'a>(conditions: &'a [serde_json::Value], type_: &str) -> Option<&'a serde_json::Value> {
    conditions.iter().find(|c| c["type"].as_str() == Some(type_))
}

impl LinkSysStatusTestSuite {
    // =========================================================================
    // Scenario 1: valid Redis LinkSys → Accepted=True
    // =========================================================================

    fn linksys_valid_redis_accepted() -> TestCase {
        TestCase::new(
            "linksys_valid_redis_accepted",
            "LinkSys 'ls-status-redis-valid': Accepted=True for valid Redis config",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let name = "ls-status-redis-valid";

                    match fetch_conditions(&ctx, name).await {
                        Err(e) => TestResult::failed(start.elapsed(), e),
                        Ok(conditions) => match find_condition(&conditions, "Accepted") {
                            Some(c) if c["status"].as_str() == Some("True") => {
                                TestResult::passed_with_message(start.elapsed(), "Accepted=True".to_string())
                            }
                            Some(c) => TestResult::failed(start.elapsed(), format!("Accepted not True: {:?}", c)),
                            None => TestResult::failed(
                                start.elapsed(),
                                format!("Accepted condition not found: {:?}", conditions),
                            ),
                        },
                    }
                })
            },
        )
    }

    // =========================================================================
    // Scenario 2: Redis with empty endpoints → Accepted=False/Invalid
    // =========================================================================

    fn linksys_invalid_empty_endpoints_rejected() -> TestCase {
        TestCase::new(
            "linksys_invalid_empty_endpoints_rejected",
            "LinkSys 'ls-status-redis-empty-endpoints': Accepted=False/Invalid, message contains 'endpoints'",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let name = "ls-status-redis-empty-endpoints";

                    let url = format!(
                        "{}/configserver/LinkSys?namespace={}&name={}",
                        ctx.admin_api_url(),
                        NS,
                        name
                    );

                    let mut accepted_false_found = false;
                    let mut last_err = String::new();

                    for _ in 0..20 {
                        let mut req = ctx.http_client.get(&url);
                        if let Some(ref token) = ctx.admin_token {
                            req = req.header("Authorization", format!("Bearer {}", token));
                        }

                        match req.send().await {
                            Ok(resp) if resp.status().is_success() => match resp.json::<serde_json::Value>().await {
                                Ok(body) if body["success"].as_bool().unwrap_or(false) => {
                                    if let Some(conditions) = body["data"]["status"]["conditions"].as_array() {
                                        if let Some(c) = find_condition(conditions, "Accepted") {
                                            if c["status"].as_str() == Some("False")
                                                && c["reason"].as_str() == Some("Invalid")
                                            {
                                                let msg = c["message"].as_str().unwrap_or("");
                                                if msg.to_lowercase().contains("endpoint") {
                                                    accepted_false_found = true;
                                                    break;
                                                } else {
                                                    last_err = format!(
                                                        "Accepted=False/Invalid but message lacks 'endpoint': '{}'",
                                                        msg
                                                    );
                                                }
                                            } else {
                                                last_err = format!("Accepted not False/Invalid: {:?}", c);
                                            }
                                        } else {
                                            last_err = "Accepted condition not found".to_string();
                                        }
                                    } else {
                                        last_err = "status.conditions not present".to_string();
                                    }
                                }
                                Ok(body) => {
                                    last_err = format!("API error: {}", body["error"].as_str().unwrap_or("unknown"));
                                }
                                Err(e) => {
                                    last_err = format!("JSON parse error: {}", e);
                                }
                            },
                            Ok(resp) => {
                                last_err = format!("HTTP {}", resp.status());
                            }
                            Err(e) => {
                                last_err = format!("Request error: {}", e);
                            }
                        }

                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    }

                    if accepted_false_found {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "Accepted=False/Invalid with 'endpoints' in message".to_string(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "Expected Accepted=False/Invalid for empty-endpoints Redis. Last: {}",
                                last_err
                            ),
                        )
                    }
                })
            },
        )
    }
}

impl TestSuite for LinkSysStatusTestSuite {
    fn name(&self) -> &str {
        "LinkSys/Status"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::linksys_valid_redis_accepted(),
            Self::linksys_invalid_empty_endpoints_rejected(),
        ]
    }
}
