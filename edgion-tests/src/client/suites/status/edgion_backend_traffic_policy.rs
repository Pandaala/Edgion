//! GEP-2648 ancestor-status test suite for EdgionBackendTrafficPolicy.
//!
//! Fixture layout (edgion-tests/integration/conf/Status/EdgionBackendTrafficPolicy/):
//! - Gateway_edgion-test_ebtp-anc-gw-a.yaml / Gateway_edgion-test_ebtp-anc-gw-b.yaml
//! - HTTPRoute_edgion-test_ebtp-anc-route-a.yaml / HTTPRoute_edgion-test_ebtp-anc-route-b.yaml
//! - Service_edgion-test_ebtp-anc-svc-multi.yaml (reachable via both routes)
//! - Service_edgion-test_ebtp-anc-svc-a-only.yaml (reachable via route-a only)
//! - Service_edgion-test_ebtp-anc-svc-b-only.yaml (reachable via route-b only)
//! - EdgionBackendTrafficPolicy_edgion-test_ebtp-anc-a-win-multi.yaml (targets svc-multi)
//! - EdgionBackendTrafficPolicy_edgion-test_ebtp-anc-b-win-bonly.yaml (targets svc-b-only)
//! - EdgionBackendTrafficPolicy_edgion-test_ebtp-anc-z-loser.yaml (targets svc-a-only + svc-b-only)
//!
//! NOTE on conflict ordering: the FS conf center yields no creationTimestamp,
//! so oldest-wins falls back to the name tie-break — winners are named
//! `ebtp-anc-a-*`/`ebtp-anc-b-*` and the loser `ebtp-anc-z-*` (same trick as
//! the control-plane `Status/BackendTLSPolicy` conflict fixtures).

use super::helpers;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use serde_json::Value;
use std::time::{Duration, Instant};

pub struct EbtpAncestorStatusTestSuite;

const POLICY_NAMESPACE: &str = "edgion-test";

/// (status, reason) of condition `ctype` on the ancestor whose
/// ancestorRef.name == `gw_name`; None if the ancestor or condition is absent.
fn ancestor_condition(policy: &Value, gw_name: &str, ctype: &str) -> Option<(String, String)> {
    policy["status"]["ancestors"].as_array()?.iter().find_map(|a| {
        if a["ancestorRef"]["name"].as_str() != Some(gw_name) {
            return None;
        }
        a["conditions"].as_array()?.iter().find_map(|c| {
            (c["type"].as_str() == Some(ctype)).then(|| {
                (
                    c["status"].as_str().unwrap_or_default().to_string(),
                    c["reason"].as_str().unwrap_or_default().to_string(),
                )
            })
        })
    })
}

fn ancestor_names(policy: &Value) -> Vec<String> {
    policy["status"]["ancestors"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x["ancestorRef"]["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

impl EbtpAncestorStatusTestSuite {
    fn winner_reports_two_ancestors() -> TestCase {
        TestCase::new(
            "winner_reports_two_ancestors",
            "ebtp-anc-a-win-multi (sole owner of svc-multi) should report both ancestor Gateways as Accepted=True/ResolvedRefs=True",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    tokio::time::sleep(Duration::from_millis(1000)).await;

                    match helpers::fetch_resource_status(
                        &ctx,
                        "EdgionBackendTrafficPolicy",
                        POLICY_NAMESPACE,
                        "ebtp-anc-a-win-multi",
                    )
                    .await
                    {
                        Ok(policy) => {
                            let mut names = ancestor_names(&policy);
                            names.sort();
                            if names != vec!["ebtp-anc-gw-a".to_string(), "ebtp-anc-gw-b".to_string()] {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected ancestors [ebtp-anc-gw-a, ebtp-anc-gw-b], got: {}. Full status: {}",
                                        names.join(", "),
                                        serde_json::to_string_pretty(&policy["status"]).unwrap_or_default()
                                    ),
                                );
                            }

                            for gw in ["ebtp-anc-gw-a", "ebtp-anc-gw-b"] {
                                let accepted = ancestor_condition(&policy, gw, "Accepted");
                                if accepted != Some(("True".to_string(), "Accepted".to_string())) {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!(
                                            "Expected Accepted=(True, Accepted) on {}, got: {:?}. Full status: {}",
                                            gw,
                                            accepted,
                                            serde_json::to_string_pretty(&policy["status"]).unwrap_or_default()
                                        ),
                                    );
                                }
                                let resolved = ancestor_condition(&policy, gw, "ResolvedRefs");
                                match &resolved {
                                    Some((status, _)) if status == "True" => {}
                                    _ => {
                                        return TestResult::failed(
                                            start.elapsed(),
                                            format!(
                                                "Expected ResolvedRefs=True on {}, got: {:?}. Full status: {}",
                                                gw,
                                                resolved,
                                                serde_json::to_string_pretty(&policy["status"]).unwrap_or_default()
                                            ),
                                        );
                                    }
                                }
                            }

                            TestResult::passed_with_message(
                                start.elapsed(),
                                "Both ancestors Accepted=True/ResolvedRefs=True as expected".to_string(),
                            )
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    fn every_ancestor_stamped_with_controller_name() -> TestCase {
        TestCase::new(
            "every_ancestor_stamped_with_controller_name",
            "Every ancestor entry on ebtp-anc-a-win-multi should carry a non-empty controllerName and ancestorRef.kind=Gateway",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    tokio::time::sleep(Duration::from_millis(1000)).await;

                    match helpers::fetch_resource_status(
                        &ctx,
                        "EdgionBackendTrafficPolicy",
                        POLICY_NAMESPACE,
                        "ebtp-anc-a-win-multi",
                    )
                    .await
                    {
                        Ok(policy) => {
                            let ancestors = match policy["status"]["ancestors"].as_array() {
                                Some(a) if !a.is_empty() => a,
                                _ => {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!(
                                            "Expected non-empty status.ancestors, got status: {}",
                                            serde_json::to_string_pretty(&policy["status"]).unwrap_or_default()
                                        ),
                                    );
                                }
                            };

                            for (i, a) in ancestors.iter().enumerate() {
                                let controller_name = a["controllerName"].as_str().unwrap_or_default();
                                if controller_name.is_empty() {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!(
                                            "ancestors[{}].controllerName is empty. Full status: {}",
                                            i,
                                            serde_json::to_string_pretty(&policy["status"]).unwrap_or_default()
                                        ),
                                    );
                                }
                                let kind = a["ancestorRef"]["kind"].as_str().unwrap_or_default();
                                if kind != "Gateway" {
                                    return TestResult::failed(
                                        start.elapsed(),
                                        format!(
                                            "ancestors[{}].ancestorRef.kind = {:?}, expected \"Gateway\". Full status: {}",
                                            i,
                                            kind,
                                            serde_json::to_string_pretty(&policy["status"]).unwrap_or_default()
                                        ),
                                    );
                                }
                            }

                            TestResult::passed_with_message(
                                start.elapsed(),
                                format!(
                                    "All {} ancestor(s) stamped with a non-empty controllerName and kind=Gateway",
                                    ancestors.len()
                                ),
                            )
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    fn loser_conflicted_on_affected_ancestor_only() -> TestCase {
        TestCase::new(
            "loser_conflicted_on_affected_ancestor_only",
            "ebtp-anc-z-loser should be Accepted=True on gw-a (sole owner of svc-a-only) and Accepted=False/Conflicted on gw-b (lost svc-b-only to ebtp-anc-b-win-bonly)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    tokio::time::sleep(Duration::from_millis(1000)).await;

                    match helpers::fetch_resource_status(
                        &ctx,
                        "EdgionBackendTrafficPolicy",
                        POLICY_NAMESPACE,
                        "ebtp-anc-z-loser",
                    )
                    .await
                    {
                        Ok(policy) => {
                            let on_a = ancestor_condition(&policy, "ebtp-anc-gw-a", "Accepted");
                            if on_a != Some(("True".to_string(), "Accepted".to_string())) {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected Accepted=(True, Accepted) on ebtp-anc-gw-a, got: {:?}. Full status: {}",
                                        on_a,
                                        serde_json::to_string_pretty(&policy["status"]).unwrap_or_default()
                                    ),
                                );
                            }
                            let on_b = ancestor_condition(&policy, "ebtp-anc-gw-b", "Accepted");
                            if on_b != Some(("False".to_string(), "Conflicted".to_string())) {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected Accepted=(False, Conflicted) on ebtp-anc-gw-b, got: {:?}. Full status: {}",
                                        on_b,
                                        serde_json::to_string_pretty(&policy["status"]).unwrap_or_default()
                                    ),
                                );
                            }

                            TestResult::passed_with_message(
                                start.elapsed(),
                                "gw-a Accepted=True, gw-b Accepted=False/Conflicted as expected".to_string(),
                            )
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    fn single_gateway_winner_has_one_ancestor() -> TestCase {
        TestCase::new(
            "single_gateway_winner_has_one_ancestor",
            "ebtp-anc-b-win-bonly should report exactly one ancestor (gw-b), Accepted=True",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    tokio::time::sleep(Duration::from_millis(1000)).await;

                    match helpers::fetch_resource_status(
                        &ctx,
                        "EdgionBackendTrafficPolicy",
                        POLICY_NAMESPACE,
                        "ebtp-anc-b-win-bonly",
                    )
                    .await
                    {
                        Ok(policy) => {
                            let names = ancestor_names(&policy);
                            if names != vec!["ebtp-anc-gw-b".to_string()] {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected ancestors [ebtp-anc-gw-b], got: {:?}. Full status: {}",
                                        names,
                                        serde_json::to_string_pretty(&policy["status"]).unwrap_or_default()
                                    ),
                                );
                            }
                            let accepted = ancestor_condition(&policy, "ebtp-anc-gw-b", "Accepted");
                            if accepted != Some(("True".to_string(), "Accepted".to_string())) {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected Accepted=(True, Accepted) on ebtp-anc-gw-b, got: {:?}. Full status: {}",
                                        accepted,
                                        serde_json::to_string_pretty(&policy["status"]).unwrap_or_default()
                                    ),
                                );
                            }

                            TestResult::passed_with_message(
                                start.elapsed(),
                                "Single ancestor gw-b, Accepted=True as expected".to_string(),
                            )
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for EbtpAncestorStatusTestSuite {
    fn name(&self) -> &str {
        "EdgionBackendTrafficPolicy Status"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::winner_reports_two_ancestors(),
            Self::every_ancestor_stamped_with_controller_name(),
            Self::loser_conflicted_on_affected_ancestor_only(),
            Self::single_gateway_winner_has_one_ancestor(),
        ]
    }
}
