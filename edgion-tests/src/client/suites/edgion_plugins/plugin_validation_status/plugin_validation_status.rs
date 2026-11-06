// EdgionPlugins Plugin Validation Status Integration Test Suite
//
// Verifies that the controller's preparse step correctly detects invalid plugin
// configurations and surfaces them as Accepted=False/Invalid on EdgionPlugins.
//
// Required conf files (edgion-tests/integration/conf/EdgionPlugins/PluginValidationStatus/):
//   01_EdgionPlugins_ep-validation-jwt-auth-invalid.yaml        — JwtAuth: no credential + no anonymous
//   02_EdgionPlugins_ep-validation-cors-invalid.yaml            — Cors: allowCredentials=true + wildcard origin
//   03_EdgionPlugins_ep-validation-direct-endpoint-invalid.yaml — DirectEndpoint: invalid extract regex
//   04_EdgionPlugins_ep-validation-rate-limit-invalid.yaml      — RateLimitLocal: rate=0
//   05_EdgionPlugins_ep-validation-alias-invalid.yaml           — Cors entry with alias containing a space
//   06_EdgionPlugins_ep-validation-dsl-invalid.yaml             — Dsl: uncompilable inline source
//   07_EdgionPlugins_ep-validation-cors-regex-invalid.yaml      — Cors: allowCredentials=true + catch-all allowOriginsByRegex
//   08_EdgionPlugins_ep-validation-fault-injection-invalid.yaml — FaultInjection: ratio out of range + mutual exclusion
//   10_EdgionPlugins_ep-validation-dsl-sync-response-stage.yaml — Dsl: placed in the sync upstreamResponseFilterPlugins stage
//
// Scenarios:
//   1. jwt_auth_invalid          → Accepted=False/Invalid, message contains "issuerGroups"
//   2. cors_invalid              → Accepted=False/Invalid, message contains "allow_credentials"
//   3. direct_ep_invalid         → Accepted=False/Invalid, message contains "regex"
//   4. rate_limit_invalid        → Accepted=False/Invalid, message contains "rate"
//   5. alias_invalid             → Accepted=False/Invalid, message contains "alias"
//   6. dsl_invalid               → Accepted=False/Invalid, message contains "DSL compile errors"
//   7. cors_regex_invalid        → Accepted=False/Invalid, message contains "allow_origins_by_regex"
//   8. fault_injection_invalid   → Accepted=False/Invalid, message contains "delay.ratio"
//   9. condition_set_structural_invalid → malformed bounded boolean shapes are rejected by the Admin API
//  10. condition_set_empty_invalid      → an empty operator is retained with Accepted=False/Invalid
//  11. dsl_sync_response_stage_invalid  → Accepted=False/Invalid, message names the wrong stage

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use crate::suites::status::helpers::{controller_local_conditions, expect_admin_yaml_admission_rejected};
use std::time::Instant;

const NS: &str = "default";
const K8S_NS: &str = "edgion-default";

fn k8s_mode() -> bool {
    std::env::var("EDGION_TEST_K8S_MODE").as_deref() == Ok("true")
}

fn test_namespace() -> &'static str {
    if k8s_mode() {
        K8S_NS
    } else {
        NS
    }
}

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
            test_namespace(),
            name
        ));
        if let Some(ref token) = ctx.admin_token {
            req = req.header("Authorization", format!("Bearer {}", token));
        }

        match req.send().await {
            Ok(resp) if resp.status().is_success() => match resp.json::<serde_json::Value>().await {
                Ok(body) if body["success"].as_bool().unwrap_or(false) => {
                    if let Some(conditions) = controller_local_conditions(&body["data"]["status"]) {
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
                    async move { assert_accepted_false(&ctx, "ep-validation-jwt-auth-invalid", "issuerGroups").await },
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
    // Scenario 4: RateLimitLocal with rate=0 → Accepted=False/Invalid
    // =========================================================================

    fn rate_limit_invalid() -> TestCase {
        TestCase::new(
            "rate_limit_invalid",
            "RateLimitLocal 'ep-validation-rate-limit-invalid': rate=0 → Accepted=False/Invalid",
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
    // (DslConfig::compile_and_validate → edgion_dsl::resolve_script →
    // compile_dsl_source). An uncompilable `source` is reported as
    // "DSL compile errors: ..." which the controller joins verbatim into the
    // Accepted condition message.
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
    // Dsl in the synchronous upstreamResponseFilterPlugins stage
    //             → Accepted=False/Invalid
    //
    // DSL response scripts run only in the async upstreamResponsePlugins stage;
    // the shared stage table rejects the synchronous placement at preparse.
    // =========================================================================

    fn dsl_sync_response_stage_invalid() -> TestCase {
        TestCase::new(
            "dsl_sync_response_stage_invalid",
            "Dsl 'ep-validation-dsl-sync-response-stage': sync response stage → Accepted=False/Invalid",
            |ctx: TestContext| {
                Box::pin(async move {
                    assert_accepted_false(
                        &ctx,
                        "ep-validation-dsl-sync-response-stage",
                        "(Dsl): plugin is not valid for upstream response filter stage",
                    )
                    .await
                })
            },
        )
    }

    // =========================================================================
    // Scenario 7: Cors with allowCredentials=true + catch-all allowOriginsByRegex
    //             → Accepted=False/Invalid
    //
    // A broad regex (".*") must not bypass the allow_credentials wildcard guard.
    // The runtime error message uses snake_case "allow_origins_by_regex".
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

    // =========================================================================
    // Scenario 8: FaultInjection with delay.ratio out of [0.0, 1.0]
    //             → Accepted=False/Invalid
    //
    // The first validation error hit is the ratio range check:
    //   "delay.ratio must be in [0.0, 1.0], got 2"
    // The keyword "delay.ratio" is a safe substring assertion.
    // =========================================================================

    fn fault_injection_invalid() -> TestCase {
        TestCase::new(
            "fault_injection_invalid",
            "FaultInjection 'ep-validation-fault-injection-invalid': ratio out of range → Accepted=False/Invalid",
            |ctx: TestContext| {
                Box::pin(async move {
                    assert_accepted_false(&ctx, "ep-validation-fault-injection-invalid", "delay.ratio").await
                })
            },
        )
    }

    fn condition_set_structural_invalid() -> TestCase {
        TestCase::new(
            "condition_set_structural_invalid",
            "ConditionSet rejects both operators and forbidden recursive shapes",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    if k8s_mode() {
                        if std::env::var("EDGION_TEST_K8S_ADMISSION_VERIFIED").as_deref()
                            == Ok("PluginValidationStatus")
                        {
                            return TestResult::passed_with_message(
                                start.elapsed(),
                                "Kubernetes server dry-run rejected all malformed ConditionSet shapes".to_string(),
                            );
                        }
                        return TestResult::failed(
                            start.elapsed(),
                            "Kubernetes admission verification flag is missing".to_string(),
                        );
                    }
                    let cases = [
                        (
                            "both-operators",
                            "allOf:\n            - type: probability\n              ratio: 1.0\n          anyOf:\n            - type: probability\n              ratio: 1.0",
                        ),
                        (
                            "allof-to-anyof",
                            "allOf:\n            - anyOf:\n                - type: probability\n                  ratio: 1.0",
                        ),
                        (
                            "anyof-to-anyof",
                            "anyOf:\n            - anyOf:\n                - type: probability\n                  ratio: 1.0",
                        ),
                    ];

                    for (suffix, condition_set) in cases {
                        let name = format!("ep-validation-condition-set-{suffix}");
                        let yaml = format!(
                            "apiVersion: edgion.io/v1\nkind: EdgionPlugins\nmetadata:\n  name: {name}\n  namespace: {NS}\nspec:\n  requestPlugins:\n    - enable: true\n      type: Cors\n      config: {{}}\n      conditions:\n        run:\n          {condition_set}\n"
                        );
                        if let Err(error) = expect_admin_yaml_admission_rejected(&ctx, &yaml).await {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("ConditionSet structural case '{suffix}': {error}"),
                            );
                        }
                    }

                    TestResult::passed_with_message(
                        start.elapsed(),
                        "Both operators and forbidden recursive shapes were rejected".to_string(),
                    )
                })
            },
        )
    }

    fn condition_set_empty_invalid() -> TestCase {
        TestCase::new(
            "condition_set_empty_invalid",
            "ConditionSet empty allOf is surfaced as Accepted=False/Invalid",
            |ctx: TestContext| {
                Box::pin(async move {
                    const NAME: &str = "ep-validation-condition-set-empty";
                    const YAML: &str = r#"apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: ep-validation-condition-set-empty
  namespace: default
spec:
  requestPlugins:
    - enable: true
      type: Cors
      config: {}
      conditions:
        run:
          allOf: []
"#;

                    let start = Instant::now();
                    if !k8s_mode() {
                        if let Err(error) = ctx.apply_yaml(YAML).await {
                            return TestResult::failed(
                                start.elapsed(),
                                format!("Failed to apply empty ConditionSet: {error}"),
                            );
                        }
                    }
                    let result = assert_accepted_false(&ctx, NAME, "allOf must be non-empty").await;
                    if !k8s_mode() {
                        let _ = ctx.delete_resource("EdgionPlugins", NS, NAME).await;
                    }
                    result
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
            Self::dsl_sync_response_stage_invalid(),
            Self::cors_regex_invalid(),
            Self::fault_injection_invalid(),
            Self::condition_set_structural_invalid(),
            Self::condition_set_empty_invalid(),
        ]
    }
}
