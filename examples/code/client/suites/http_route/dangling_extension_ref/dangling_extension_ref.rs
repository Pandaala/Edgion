// HTTPRoute scenario: dangling ExtensionRef filter.
//
// Locks in the per-route ExtensionRef "not found" semantics introduced by the
// unified plugin snapshot work (Tasks 1-12). The route's filter list contains
// an `ExtensionRef` pointing to a non-existent EdgionPlugins
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
// Config: examples/test/conf/HTTPRoute/DanglingExtensionRef/

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

const HOST: &str = "dangling-extension-ref.example.com";

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
}

impl TestSuite for DanglingExtensionRefTestSuite {
    fn name(&self) -> &str {
        "HTTPRoute Dangling ExtensionRef Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_dangling_extension_ref_returns_500()]
    }
}
