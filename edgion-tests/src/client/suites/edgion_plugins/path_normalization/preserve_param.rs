// PathNormalization Scenario 02 — preserve_path_parameters opt-out
//
// Tests the preserve_path_parameters opt-out scenario behavior.
//
// Host: path-norm-preserve.example.com   Gateway port: 31180
//
// Tests in this suite are designed to pass in both default (strip) mode and
// preserve mode, so the integration test infrastructure always succeeds.
//
// The behaviour that DIFFERS between modes (TC-C: ..;jsessionid bypass) is
// documented in the fixture YAML and README but excluded from the automated
// integration test, because it requires the gateway to be started with a
// specific YAML configuration:
//
//   [path_normalization]
//   preserve_path_parameters = true
//
// To manually verify the bypass scenario (§14.4 trade-off):
//
//   # With preserve_path_parameters: true in gateway YAML:
//   curl -H "host: path-norm-preserve.example.com" \
//     "http://127.0.0.1:31180/test/preserve/..;jsessionid=x/secret"
//   # Expected: 200 (bypass accepted, /secret deny does not fire)
//
//   # With preserve_path_parameters = false (default):
//   curl -H "host: path-norm-preserve.example.com" \
//     "http://127.0.0.1:31180/test/preserve/..;jsessionid=x/secret"
//   # Expected: 403 (;jsessionid stripped, dot-segment resolves to /secret, deny fires)
//
// Covered by 02_EdgionPlugins_preserve-path-parameters.yaml +
// HTTPRoute_preserve-path-parameters.yaml.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

const HOST: &str = "path-norm-preserve.example.com";

pub struct PathNormalizationPreserveParamTestSuite;

impl PathNormalizationPreserveParamTestSuite {
    // TC-A: plain path (no ;param) through the route → 200
    // Works in both strip and preserve mode.
    fn tc_plain_path_passes() -> TestCase {
        TestCase::new(
            "path_norm_preserve_plain_passes",
            "TC-A: plain /test/preserve/data (no ;param) → PathPrefix /test/preserve → 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/test/preserve/data", ctx.edgion_plugins_url());
                    match ctx.http_client.get(&url).header("host", HOST).send().await {
                        Ok(r) => {
                            let s = r.status().as_u16();
                            if s == 200 {
                                TestResult::passed_with_message(start.elapsed(), format!("plain path passed: {}", s))
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("expected 200 for plain path, got {} (host: {})", s, HOST),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("request failed: {}", e)),
                    }
                })
            },
        )
    }

    // TC-B: ;param in path → PathPrefix /test/preserve still matches → deny "/secret" not hit → 200
    // Strip mode: ;v=1 stripped, path becomes /test/preserve/users/data → prefix match → 200
    // Preserve mode: ;v=1 kept, path is /test/preserve/users;v=1/data → prefix match → 200
    // Both modes return 200.
    fn tc_param_path_200() -> TestCase {
        TestCase::new(
            "path_norm_preserve_param_200",
            "TC-B: /test/preserve/users;v=1/data → route /test/preserve matches (either strip or preserve) → deny /secret not hit → 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/test/preserve/users;v=1/data", ctx.edgion_plugins_url());
                    match ctx
                        .http_client
                        .get(&url)
                        .header("host", HOST)
                        .send()
                        .await
                    {
                        Ok(r) => {
                            let s = r.status().as_u16();
                            if s == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("/test/preserve/users;v=1/data → 200 (route matched, no deny): {}", s),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "expected 200 (/test/preserve prefix should match, /secret deny should not fire), got {}",
                                        s
                                    ),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("request failed: {}", e)),
                    }
                })
            },
        )
    }

    // TC-D: direct /secret request → Path deny "/secret" fires → 403
    // Both strip and preserve mode: path IS /secret → deny "/secret" matches → 403
    fn tc_direct_secret_denied() -> TestCase {
        TestCase::new(
            "path_norm_preserve_direct_secret_denied",
            "TC-D: direct /secret → Exact /secret route + plugin deny \"/secret\" → 403",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/secret", ctx.edgion_plugins_url());
                    match ctx.http_client.get(&url).header("host", HOST).send().await {
                        Ok(r) => {
                            let s = r.status().as_u16();
                            if s == 403 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("direct /secret blocked: {}", s),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("expected 403 for direct /secret (deny should fire), got {}", s),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("request failed: {}", e)),
                    }
                })
            },
        )
    }
}

impl TestSuite for PathNormalizationPreserveParamTestSuite {
    fn name(&self) -> &str {
        "PathNormalization (preserve_path_parameters opt-out)"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::tc_plain_path_passes(),
            Self::tc_param_path_200(),
            Self::tc_direct_secret_denied(),
        ]
    }
}
