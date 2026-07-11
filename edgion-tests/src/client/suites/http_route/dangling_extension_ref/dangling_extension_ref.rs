// HTTPRoute scenario: dangling ExtensionRef filter.
//
// Locks in the per-route ExtensionRef "not found" semantics. The route's filter
// list contains an `ExtensionRef` pointing to a non-existent EdgionPlugins
// (`no-such-plugin`). At route build time, the resolved slot for this filter
// is None.
//
// When the request hits the async Request stage, ExtensionRefFilter's
// prepare_execution finds the resolved slot empty, pushes
// "extension ref not found" to the PluginLog, and returns
// ErrTerminateRequest. The async path then writes a 500 + JSON body
// (`{"message":"plugin reference error"}`) via
// write_extension_ref_error_response — see
// `src/core/gateway/plugins/runtime/gateway_api_filters/extension_ref.rs`.
//
// Config: edgion-tests/integration/conf/HTTPRoute/DanglingExtensionRef/

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

const HOST: &str = "dangling-extension-ref.example.com";

// The HTTPRoutes in this scenario are stored under the `edgion-default`
// namespace (see the conf YAMLs' metadata.namespace). The configserver
// admin API query must use the same namespace.
const ROUTE_NS: &str = "edgion-default";

pub struct DanglingExtensionRefTestSuite;

impl DanglingExtensionRefTestSuite {
    // Dangling ExtensionRef on async Request stage -> 500 + plugin reference error body.
    fn test_dangling_extension_ref_returns_500() -> TestCase {
        TestCase::new(
            "dangling_extension_ref_returns_500",
            "Dangling ExtensionRef in HTTPRoute filter returns 500 + plugin reference error body",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let resp = ctx
                        .http_client
                        .get(format!("{}/dangling", ctx.http_url()))
                        .header("host", HOST)
                        .send()
                        .await;

                    match resp {
                        Ok(resp) => {
                            let status = resp.status().as_u16();
                            if status != 500 {
                                return TestResult::failed(start.elapsed(), format!("Expected 500, got {}", status));
                            }
                            let body = match resp.text().await {
                                Ok(b) => b,
                                Err(e) => {
                                    return TestResult::failed(start.elapsed(), format!("Failed to read body: {}", e));
                                }
                            };
                            if !body.contains("plugin reference error") {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected body to contain 'plugin reference error', got: {}", body),
                                );
                            }
                            TestResult::passed(start.elapsed())
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // An ExtensionRef filter whose `kind` is not `EdgionPlugins` (the only
    // supported ExtensionRef kind) is flagged at the controller's preparse step:
    // the rule index goes into `invalid_rule_indices` and the HTTPRoute gets a
    // `PartiallyInvalid=True` condition. This polls the configserver admin API
    // for that condition.
    fn unsupported_extension_ref_kind_marks_route_partially_invalid() -> TestCase {
        TestCase::new(
            "unsupported_extension_ref_kind_marks_route_partially_invalid",
            "HTTPRoute with an unsupported ExtensionRef kind gets PartiallyInvalid=True",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let name = "unsupported-extension-ref-kind-route";
                    let url = format!(
                        "{}/configserver/HTTPRoute?namespace={}&name={}",
                        ctx.admin_api_url(),
                        ROUTE_NS,
                        name
                    );

                    let mut last_err = String::from("no response");

                    for _ in 0..20 {
                        let mut req = ctx.http_client.get(&url);
                        if let Some(ref token) = ctx.admin_token {
                            req = req.header("Authorization", format!("Bearer {}", token));
                        }

                        match req.send().await {
                            Ok(resp) if resp.status().is_success() => {
                                match resp.json::<serde_json::Value>().await {
                                    Ok(body) if body["success"].as_bool().unwrap_or(false) => {
                                        if let Some(conditions) = body["data"]["status"]["conditions"].as_array() {
                                            if let Some(cond) = conditions
                                                .iter()
                                                .find(|c| c["type"].as_str() == Some("PartiallyInvalid"))
                                            {
                                                if cond["status"].as_str() == Some("True") {
                                                    // Best-effort: the preparse reason names the
                                                    // unsupported kind. Don't fail if absent.
                                                    let msg = cond["message"].as_str().unwrap_or("");
                                                    let note = if msg.contains("NotAPlugin") {
                                                        "message names the unsupported kind"
                                                    } else {
                                                        "PartiallyInvalid=True (message did not name the kind)"
                                                    };
                                                    return TestResult::passed_with_message(
                                                        start.elapsed(),
                                                        note.to_string(),
                                                    );
                                                }
                                                last_err =
                                                    format!("PartiallyInvalid present but status != True: {:?}", cond);
                                            } else {
                                                last_err = "status.conditions lacks PartiallyInvalid".to_string();
                                            }
                                        } else {
                                            last_err = "status.conditions not present".to_string();
                                        }
                                    }
                                    Ok(body) => {
                                        last_err =
                                            format!("API error: {}", body["error"].as_str().unwrap_or("unknown"));
                                    }
                                    Err(e) => {
                                        last_err = format!("JSON parse error: {}", e);
                                    }
                                }
                            }
                            Ok(resp) => {
                                last_err = format!("HTTP {}", resp.status());
                            }
                            Err(e) => {
                                last_err = format!("Request error: {}", e);
                            }
                        }

                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    }

                    TestResult::failed(
                        start.elapsed(),
                        format!("Expected PartiallyInvalid=True for '{}'. Last: {}", name, last_err),
                    )
                })
            },
        )
    }
}

impl TestSuite for DanglingExtensionRefTestSuite {
    fn name(&self) -> &str {
        "HTTPRoute Dangling ExtensionRef Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_dangling_extension_ref_returns_500(),
            Self::unsupported_extension_ref_kind_marks_route_partially_invalid(),
        ]
    }
}
