// PathNormalization Scenario 01 Test Suite
//
// Tests the default path normalization behavior (preserve_path_parameters = false).
//
// Host: path-norm.example.com  Gateway port: 31180
//
// Covered by 01_EdgionPlugins_request-restriction-path-deny.yaml +
// HTTPRoute_default_path-normalization.yaml.
//
// ## What is tested:
//
// TC-3: HTTPRoute Exact /admin matches /%61dmin (normalization before routing)
//   /%61dmin → percent-decoded to /admin → Exact /admin rule matches → 200
//
// TC-4: HTTPRoute PathPrefix /api matches /api//v1//users (double-slash collapse)
//   /api//v1//users → slashes collapsed → /api/v1/users → PathPrefix /api matches → 200
//
// TC-6: Path-parameter bypass (..;jsessionid) — strip mode (default)
//   /admin/..;jsessionid=abc/secret → strip ;jsessionid → /admin/../secret → /secret
//   Exact /secret rule matches → 200

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

const HOST: &str = "path-norm.example.com";

pub struct PathNormalizationTestSuite;

impl PathNormalizationTestSuite {
    // TC-3a: /%61dmin → Exact /admin → 200
    // %61 decodes to 'a', so /%61dmin → /admin
    fn tc3_encoded_admin_routes() -> TestCase {
        TestCase::new(
            "path_norm_encoded_admin_routes",
            "TC-3: /%61dmin normalised to /admin → Exact /admin route matches → 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    // %61 = 'a', so /%61dmin decodes to /admin
                    let url = format!("{}/%61dmin", ctx.edgion_plugins_url());
                    match ctx.http_client.get(&url).header("host", HOST).send().await {
                        Ok(r) => {
                            let s = r.status().as_u16();
                            if s == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("/%61dmin normalised to /admin, Exact route matched: {}", s),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("expected 200, got {}", s))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("request failed: {}", e)),
                    }
                })
            },
        )
    }

    // TC-3b: plain /admin routes directly
    fn tc3_plain_admin_routes() -> TestCase {
        TestCase::new(
            "path_norm_plain_admin_routes",
            "TC-3: /admin → Exact /admin route → 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/admin", ctx.edgion_plugins_url());
                    match ctx.http_client.get(&url).header("host", HOST).send().await {
                        Ok(r) => {
                            let s = r.status().as_u16();
                            if s == 200 {
                                TestResult::passed_with_message(start.elapsed(), format!("plain /admin matched: {}", s))
                            } else {
                                TestResult::failed(start.elapsed(), format!("expected 200, got {}", s))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("request failed: {}", e)),
                    }
                })
            },
        )
    }

    // TC-4: double-slash collapse
    fn tc4_double_slash_collapsed() -> TestCase {
        TestCase::new(
            "path_norm_double_slash_collapsed",
            "TC-4: /api//v1//users double-slash collapsed to /api/v1/users → PathPrefix /api → 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/api//v1//users", ctx.edgion_plugins_url());
                    match ctx.http_client.get(&url).header("host", HOST).send().await {
                        Ok(r) => {
                            let s = r.status().as_u16();
                            if s == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("double-slash collapsed, prefix matched: {}", s),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("expected 200, got {}", s))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("request failed: {}", e)),
                    }
                })
            },
        )
    }

    // TC-6: ..;jsessionid bypass is normalised to /secret in strip mode
    // /admin/..;jsessionid=abc/secret → strip path param → /admin/../secret → dot remove → /secret
    // Exact /secret route matches → 200
    fn tc6_path_param_bypass_strip_mode() -> TestCase {
        TestCase::new(
            "path_norm_path_param_strip_to_secret",
            "TC-6: /admin/..;jsessionid=abc/secret (strip mode) → normalises to /secret → Exact /secret matches → 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/admin/..;jsessionid=abc/secret", ctx.edgion_plugins_url());
                    match ctx.http_client.get(&url).header("host", HOST).send().await {
                        Ok(r) => {
                            let s = r.status().as_u16();
                            if s == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!(
                                        "..;jsessionid stripped, normalised to /secret, matched Exact route: {}",
                                        s
                                    ),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("expected 200 (normalised to /secret which has Exact route), got {}", s),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("request failed: {}", e)),
                    }
                })
            },
        )
    }

    // TC-6b: /secret direct access → 200 (the Exact /secret route always works)
    fn tc6_direct_secret() -> TestCase {
        TestCase::new(
            "path_norm_direct_secret_200",
            "TC-6b: /secret direct access → Exact /secret route → 200",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/secret", ctx.edgion_plugins_url());
                    match ctx.http_client.get(&url).header("host", HOST).send().await {
                        Ok(r) => {
                            let s = r.status().as_u16();
                            if s == 200 {
                                TestResult::passed_with_message(start.elapsed(), format!("direct /secret: {}", s))
                            } else {
                                TestResult::failed(start.elapsed(), format!("expected 200, got {}", s))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("request failed: {}", e)),
                    }
                })
            },
        )
    }
}

impl TestSuite for PathNormalizationTestSuite {
    fn name(&self) -> &str {
        "PathNormalization Default (strip mode)"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::tc3_encoded_admin_routes(),
            Self::tc3_plain_admin_routes(),
            Self::tc4_double_slash_collapsed(),
            Self::tc6_path_param_bypass_strip_mode(),
            Self::tc6_direct_secret(),
        ]
    }
}
