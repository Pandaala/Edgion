// HTTP-layer IpRestriction Plugin Test Suite — allowRefs / denyRefs via central IpList
//
// All cases use ipSource: RemoteIp so the checked IP is the resolved address,
// which is the deterministic TCP peer when no forwarded address is supplied.
// The IP groups are sourced from EdgionConfigData (IpList) objects rather than
// inlined in the plugin config, proving that refs resolve from the config store.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

pub struct IpRestrictionRefsPluginTestSuite;

impl IpRestrictionRefsPluginTestSuite {
    // 1. allowRefs hit — client IP (127.0.0.1) is in the referenced allow-list → 200.
    fn test_allow_ref_hit() -> TestCase {
        TestCase::new(
            "allow_ref_hit",
            "allowRefs: client IP in referenced allow-list is permitted (200)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/test/allow-ref-hit/api", ctx.http_url());
                    let request = ctx.http_client.get(&url).header("host", "ipref.example.com");
                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("IP in referenced allow list permitted with {}", status),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected status 200, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }

    // 2. denyRefs hit — client IP (127.0.0.1) is in the referenced block-list → 403.
    fn test_deny_ref_hit() -> TestCase {
        TestCase::new(
            "deny_ref_hit",
            "denyRefs: client IP in referenced block-list is denied (403)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/test/deny-ref-hit/api", ctx.http_url());
                    let request = ctx.http_client.get(&url).header("host", "ipref.example.com");
                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 403 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("IP in referenced block list denied with {}", status),
                                )
                            } else {
                                TestResult::failed(start.elapsed(), format!("Expected status 403, got {}", status))
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Request failed: {}", e)),
                    }
                })
            },
        )
    }
}

impl TestSuite for IpRestrictionRefsPluginTestSuite {
    fn name(&self) -> &str {
        "IpRestriction Refs Plugin Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![Self::test_allow_ref_hit(), Self::test_deny_ref_hit()]
    }
}
