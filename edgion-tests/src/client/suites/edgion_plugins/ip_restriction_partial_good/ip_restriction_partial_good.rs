// HTTP-layer IpRestriction Partial-Good Plugin Test Suite
//
// Traffic-based proof that partial-good IpList compilation (Tasks 1-8) holds on
// the data plane: an IpList whose CIDR group mixes one VALID and one INVALID
// entry still enforces the valid entry — the invalid one is skipped, not fatal,
// and the list never collapses to deny-all.
//
// All cases use ipSource: RemoteIp so the checked IP is the resolved address,
// which is the deterministic TCP peer when no forwarded address is supplied. The first two cases source the IP groups
// from EdgionConfigData (IpList) objects referenced via denyRefs; the last two
// preserve the former inline cases using dedicated referenced IpLists. Every
// group contains a deliberately invalid CIDR sibling.

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use std::time::Instant;

pub struct IpRestrictionPartialGoodPluginTestSuite;

impl IpRestrictionPartialGoodPluginTestSuite {
    // 1. partial-deny hit — the referenced IpList mixes a good CIDR (127.0.0.0/8,
    //    which covers the client) with an invalid sibling. Partial-good keeps the
    //    good CIDR, so the client (127.0.0.1) is a deny-list hit → 403. This
    //    proves the valid entry compiled despite the bad one (acceptance #1/#2).
    fn test_partial_deny_hit() -> TestCase {
        TestCase::new(
            "partial_deny_hit",
            "denyRefs: good CIDR enforced despite an invalid sibling entry (403)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/test/partial-deny-hit/api", ctx.http_url());
                    let request = ctx.http_client.get(&url).header("host", "ippg.example.com");
                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 403 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!("Good CIDR enforced despite invalid sibling; denied with {}", status),
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

    // 2. partial-deny miss — the referenced IpList mixes a good CIDR (10.0.0.0/8,
    //    which does NOT cover the client) with an invalid sibling. Dropping the
    //    bad entry leaves exactly { 10.0.0.0/8 }, NOT deny-all, so the client
    //    (127.0.0.1) misses the list and falls through to defaultAction: allow →
    //    200. This proves the list did not collapse to deny-all on the bad entry.
    fn test_partial_deny_miss() -> TestCase {
        TestCase::new(
            "partial_deny_miss",
            "denyRefs: list did not collapse to deny-all; unrelated IP allowed (200)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/test/partial-deny-miss/api", ctx.http_url());
                    let request = ctx.http_client.get(&url).header("host", "ippg.example.com");
                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!(
                                        "List did not collapse to deny-all; unrelated IP allowed with {}",
                                        status
                                    ),
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

    // 3. inline-deny hit — the dedicated referenced deny group mixes a good CIDR
    //    (127.0.0.0/8, which covers the client) with an invalid IpList sibling.
    //    Partial-good keeps the good CIDR, so the client (127.0.0.1) is a deny
    //    hit → 403. This proves a valid IpList entry compiles despite a bad
    //    sibling — without the plugin failing init (acceptance #1/#2, relocated-list path).
    fn test_inline_deny_hit() -> TestCase {
        TestCase::new(
            "inline_deny_hit",
            "Dedicated denyRefs: good CIDR enforced despite an invalid list sibling (403)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/test/inline-deny-hit/api", ctx.http_url());
                    let request = ctx.http_client.get(&url).header("host", "ippg.example.com");
                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 403 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!(
                                        "Good referenced CIDR enforced despite invalid sibling; denied with {}",
                                        status
                                    ),
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

    // 4. inline-deny miss — the dedicated referenced deny group mixes a good CIDR
    //    (10.0.0.0/8, which does NOT cover the client) with an invalid IpList
    //    sibling. Dropping the bad entry leaves exactly { 10.0.0.0/8 }, NOT
    //    deny-all, so the client (127.0.0.1) misses the list and falls through to
    //    defaultAction: allow → 200. Had the bad IpList CIDR made the plugin
    //    init_failed, it would deny ALL traffic (403); the 200 proves the plugin
    //    LOADED and did not collapse to deny-all.
    fn test_inline_deny_miss() -> TestCase {
        TestCase::new(
            "inline_deny_miss",
            "Dedicated denyRefs: plugin loaded, did not collapse to deny-all; unrelated IP allowed (200)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let url = format!("{}/test/inline-deny-miss/api", ctx.http_url());
                    let request = ctx.http_client.get(&url).header("host", "ippg.example.com");
                    match request.send().await {
                        Ok(response) => {
                            let status = response.status().as_u16();
                            if status == 200 {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    format!(
                                        "Plugin loaded, no deny-all collapse; unrelated IP allowed with {}",
                                        status
                                    ),
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

impl TestSuite for IpRestrictionPartialGoodPluginTestSuite {
    fn name(&self) -> &str {
        "IpRestriction Partial-Good Plugin Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_partial_deny_hit(),
            Self::test_partial_deny_miss(),
            Self::test_inline_deny_hit(),
            Self::test_inline_deny_miss(),
        ]
    }
}
