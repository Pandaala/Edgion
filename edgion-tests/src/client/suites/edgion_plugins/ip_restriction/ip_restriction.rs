// HTTP-layer IpRestriction Plugin Test Suite
//
// All cases use ipSource: remoteAddr so the checked IP is the deterministic TCP
// peer of the test client (127.0.0.1), independent of RealIp / XFF processing.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

pub struct IpRestrictionPluginTestSuite;

impl IpRestrictionPluginTestSuite {
    // 1. Whitelist miss — client IP (127.0.0.1) is not in the allow list → denied.
    fn test_allow_list_denies_ip_not_in_list() -> TestCase {
        TestCase::new(
            "allow_list_denies_ip_not_in_list",
            "Whitelist mode: client IP not in allow list is denied (403) [fr-types-01]",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/test/allow-miss/api", ctx.http_url());
                    let request = ctx.http_client.get(&url).header("host", "ip-restriction.example.com");
                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 403 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("IP not in allow list denied with {}", status),
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

    // 2. Whitelist hit — client IP is in the allow list → allowed.
    fn test_allow_list_permits_ip_in_list() -> TestCase {
        TestCase::new(
            "allow_list_permits_ip_in_list",
            "Whitelist mode: client IP in allow list is permitted (200)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/test/allow-hit/api", ctx.http_url());
                    let request = ctx.http_client.get(&url).header("host", "ip-restriction.example.com");
                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("IP in allow list permitted with {}", status),
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

    // 3. Blacklist hit — client IP is in the deny list → denied even with default allow.
    fn test_deny_list_blocks_ip_in_list() -> TestCase {
        TestCase::new(
            "deny_list_blocks_ip_in_list",
            "Blacklist mode: client IP in deny list is blocked (403)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/test/deny-hit/api", ctx.http_url());
                    let request = ctx.http_client.get(&url).header("host", "ip-restriction.example.com");
                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 403 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("IP in deny list blocked with {}", status),
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

    // 4. Blacklist miss — client IP not in deny list, default allow → allowed.
    fn test_deny_list_permits_ip_not_in_list() -> TestCase {
        TestCase::new(
            "deny_list_permits_ip_not_in_list",
            "Blacklist mode: client IP not in deny list falls through to default allow (200)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/test/deny-miss/api", ctx.http_url());
                    let request = ctx.http_client.get(&url).header("host", "ip-restriction.example.com");
                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("IP not in deny list allowed with {}", status),
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
}

impl TestSuite for IpRestrictionPluginTestSuite {
    fn name(&self) -> &str {
        "IpRestriction Plugin Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_allow_list_denies_ip_not_in_list(),
            Self::test_allow_list_permits_ip_in_list(),
            Self::test_deny_list_blocks_ip_in_list(),
            Self::test_deny_list_permits_ip_not_in_list(),
        ]
    }
}
