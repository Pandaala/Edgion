// EdgionPlugins Plugin Validation Status Integration Test Suite
//
// Verifies that the controller's preparse step correctly detects invalid plugin
// configurations and surfaces them as Accepted=False/Invalid on EdgionPlugins.
//
// Required conf files (examples/test/conf/EdgionPlugins/PluginValidationStatus/):
//   01_EdgionPlugins_ep-validation-jwt-auth-invalid.yaml        — JwtAuth: no credential + no anonymous
//   02_EdgionPlugins_ep-validation-cors-invalid.yaml            — Cors: allowCredentials=true + wildcard origin
//   03_EdgionPlugins_ep-validation-direct-endpoint-invalid.yaml — DirectEndpoint: invalid extract regex
//   04_EdgionPlugins_ep-validation-rate-limit-invalid.yaml      — RateLimit: rate=0
//   05_EdgionPlugins_ep-validation-alias-invalid.yaml           — Cors entry with alias containing a space
//   06_EdgionPlugins_ep-validation-dsl-invalid.yaml             — Dsl: uncompilable inline source
//   07_EdgionPlugins_ep-validation-cors-regex-invalid.yaml      — Cors: allowCredentials=true + catch-all allowOriginsByRegex
//
// Scenarios:
//   1. jwt_auth_invalid     → Accepted=False/Invalid, message contains "secretRef"
//   2. cors_invalid         → Accepted=False/Invalid, message contains "allow_credentials"
//   3. direct_ep_invalid    → Accepted=False/Invalid, message contains "regex"
//   4. rate_limit_invalid   → Accepted=False/Invalid, message contains "rate"
//   5. alias_invalid        → Accepted=False/Invalid, message contains "alias"
//   6. dsl_invalid          → Accepted=False/Invalid, message contains "DSL compile errors"
//   7. cors_regex_invalid   → Accepted=False/Invalid, message contains "allow_origins_by_regex"

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

const NS: &str = "default";

pub struct PluginValidationStatusTestSuite;

impl PluginValidationStatusTestSuite {
    pub fn new() -> Self {
        Self
    }
}

impl Default for PluginValidationStatusTestSuite {
    fn default() -> Self {
        Self::new()
    }
}

/// Find a condition by type in the conditions array.
fn find_condition<'a>(conditions: &'a [serde_json::Value], type_: &str) -> Option<&'a serde_json::Value> {
    conditions.iter().find(|c| c["type"].as_str() == Some(type_))
}

/// Poll until Accepted=False/Invalid with a message containing `keyword`.
async fn assert_accepted_false(ctx: &TestContext, name: &str, keyword: &str) -> TestResult {
    let start = Instant::now();

    let mut accepted_false_found = false;
    let mut last_err = String::new();

    for _ in 0..20 {
        let mut req = ctx.http_client.get(format!(
            "{}/configserver/EdgionPlugins?namespace={}&name={}",
            ctx.admin_api_url(),
            NS,
            name
        ));
        if let Some(ref token) = ctx.admin_token {
            req = req.header("Authorization", format!("Bearer {}", token));
        }

        match req.send().await {
            Ok(resp) if resp.status().is_success() => match resp.json::<serde_json::Value>().await {
                Ok(body) if body["success"].as_bool().unwrap_or(false) => {
                    if let Some(conditions) = body["data"]["status"]["conditions"].as_array() {
                        if let Some(accepted) = find_condition(conditions, "Accepted") {
                            if accepted["status"].as_str() == Some("False")
                                && accepted["reason"].as_str() == Some("Invalid")
                            {
                                let msg = accepted["message"].as_str().unwrap_or("");
                                if msg.contains(keyword) {
                                    accepted_false_found = true;
                                    break;
                                } else {
                                    last_err =
                                        format!("Accepted=False/Invalid but message lacks '{}': '{}'", keyword, msg);
                                }
                            } else {
                                last_err = format!("Accepted condition not False/Invalid: {:?}", accepted);
                            }
                        } else {
                            last_err = "status.conditions lacks Accepted".to_string();
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
            format!("Accepted=False/Invalid with '{}' in message", keyword),
        )
    } else {
        TestResult::failed(
            start.elapsed(),
            format!(
                "Expected Accepted=False/Invalid (keyword='{}') for '{}'. Last: {}",
                keyword, name, last_err
            ),
        )
    }
}

impl PluginValidationStatusTestSuite {
    // =========================================================================
    // Scenario 1: JwtAuth with no credential source → Accepted=False/Invalid
    // =========================================================================

    fn jwt_auth_invalid() -> TestCase {
        TestCase::new(
            "jwt_auth_invalid",
            "JwtAuth 'ep-validation-jwt-auth-invalid': no credential → Accepted=False/Invalid",
            |ctx: TestContext| {
                Box::pin(
                    async move { assert_accepted_false(&ctx, "ep-validation-jwt-auth-invalid", "secretRef").await },
                )
            },
        )
    }

    // =========================================================================
    // Scenario 2: Cors with allowCredentials=true + wildcard → Accepted=False/Invalid
    // =========================================================================

    fn cors_invalid() -> TestCase {
        TestCase::new(
            "cors_invalid",
            "Cors 'ep-validation-cors-invalid': credentials + wildcard → Accepted=False/Invalid",
            |ctx: TestContext| {
                Box::pin(async move {
                    // Runtime error message uses snake_case "allow_credentials".
                    assert_accepted_false(&ctx, "ep-validation-cors-invalid", "allow_credentials").await
                })
            },
        )
    }

    // =========================================================================
    // Scenario 3: DirectEndpoint with invalid regex → Accepted=False/Invalid
    // =========================================================================

    fn direct_endpoint_invalid() -> TestCase {
        TestCase::new(
            "direct_endpoint_invalid",
            "DirectEndpoint 'ep-validation-direct-endpoint-invalid': bad regex → Accepted=False/Invalid",
            |ctx: TestContext| {
                Box::pin(
                    async move { assert_accepted_false(&ctx, "ep-validation-direct-endpoint-invalid", "regex").await },
                )
            },
        )
    }

    // =========================================================================
    // Scenario 4: RateLimit with rate=0 → Accepted=False/Invalid
    // =========================================================================

    fn rate_limit_invalid() -> TestCase {
        TestCase::new(
            "rate_limit_invalid",
            "RateLimit 'ep-validation-rate-limit-invalid': rate=0 → Accepted=False/Invalid",
            |ctx: TestContext| {
                Box::pin(async move { assert_accepted_false(&ctx, "ep-validation-rate-limit-invalid", "rate").await })
            },
        )
    }

    // =========================================================================
    // Scenario 5: Plugin entry with invalid alias (contains space)
    //             → Accepted=False/Invalid
    // =========================================================================

    fn alias_invalid() -> TestCase {
        TestCase::new(
            "alias_invalid",
            "Entry 'ep-validation-alias-invalid': alias has a space → Accepted=False/Invalid",
            |ctx: TestContext| {
                Box::pin(async move { assert_accepted_false(&ctx, "ep-validation-alias-invalid", "alias").await })
            },
        )
    }

    // =========================================================================
    // Scenario 6: Dsl with an uncompilable inline source → Accepted=False/Invalid
    //
    // DslConfig::validation_error() runs the full data-plane validation path
    // (DslPlugin::validate_config → resolve_script → compile_dsl_source). An
    // uncompilable `source` is reported as "DSL compile errors: ..." which the
    // controller joins verbatim into the Accepted condition message.
    // =========================================================================

    fn dsl_invalid() -> TestCase {
        TestCase::new(
            "dsl_invalid",
            "Dsl 'ep-validation-dsl-invalid': uncompilable source → Accepted=False/Invalid",
            |ctx: TestContext| {
                Box::pin(
                    async move { assert_accepted_false(&ctx, "ep-validation-dsl-invalid", "DSL compile errors").await },
                )
            },
        )
    }

    // =========================================================================
    // Scenario 7: Cors with allowCredentials=true + catch-all allowOriginsByRegex
    //             → Accepted=False/Invalid
    //
    // Regression for fr-auth2-01: a broad regex (".*") must not bypass the
    // allow_credentials wildcard guard. The runtime error message uses
    // snake_case "allow_origins_by_regex".
    // =========================================================================

    fn cors_regex_invalid() -> TestCase {
        TestCase::new(
            "cors_regex_invalid",
            "Cors 'ep-validation-cors-regex-invalid': credentials + catch-all regex → Accepted=False/Invalid",
            |ctx: TestContext| {
                Box::pin(async move {
                    assert_accepted_false(&ctx, "ep-validation-cors-regex-invalid", "allow_origins_by_regex").await
                })
            },
        )
    }
}

impl TestSuite for PluginValidationStatusTestSuite {
    fn name(&self) -> &str {
        "PluginValidationStatus"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::jwt_auth_invalid(),
            Self::cors_invalid(),
            Self::direct_endpoint_invalid(),
            Self::rate_limit_invalid(),
            Self::alias_invalid(),
            Self::dsl_invalid(),
            Self::cors_regex_invalid(),
        ]
    }
}
