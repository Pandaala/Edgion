//! EdgionConfigData Cross-Namespace Reference Grant Test Suite
//!
//! Verifies that cross-namespace `EdgionConfigDataRef` entries in `EdgionPlugins`
//! (IpRestriction `allowRefs`/`denyRefs`) are governed by the reference-grant
//! validation mechanism when the flag is enabled:
//!
//! - No grant → `ResolvedRefs=False(RefNotPermitted)` (controller status)
//! - Grant present → `ResolvedRefs=True`
//! - `visibility: Cluster` → resolves without grant
//! - Same-namespace ref → always resolves (no grant needed)
//! - Dynamic grant add/delete triggers a requeue and status transition
//!
//! Required fixture files
//! (edgion-tests/integration/conf/Status/RefGrant/EdgionConfigData/):
//!   01_EdgionConfigData_edgion-default_ecd-refgrant-blocklist.yaml
//!   02_EdgionConfigData_edgion-default_ecd-refgrant-cluster-data.yaml
//!   03_EdgionConfigData_edgion-test_ecd-same-ns-data.yaml
//!   04_EdgionPlugins_edgion-test_ecd-deny-refs-cross-ns.yaml
//!   05_EdgionPlugins_edgion-test_ecd-allow-refs-cross-ns.yaml
//!   06_EdgionPlugins_edgion-test_ecd-cluster-ref-cross-ns.yaml
//!   07_EdgionPlugins_edgion-test_ecd-same-ns-ref.yaml
//!   08_EdgionConfigData_edgion-default_ecd-region-override.yaml
//!   09_EdgionConfigData_edgion-default_ecd-active-selector.yaml
//!   10_EdgionPlugins_edgion-test_ecd-override-ref-cross-ns.yaml
//!   11_EdgionPlugins_edgion-test_ecd-active-profile-ref-cross-ns.yaml
//!   12_EdgionPlugins_edgion-test_ecd-missing-ref.yaml

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use serde_json::Value;
use std::time::{Duration, Instant};

/// Namespace that owns the `EdgionPlugins` resources (the referrer).
const PLUGIN_NS: &str = "edgion-test";
/// Namespace that owns the `EdgionConfigData` resources (the target).
const DATA_NS: &str = "edgion-default";

/// `ReferenceGrant` in `DATA_NS` permitting `EdgionPlugins` from `PLUGIN_NS` to
/// reference any `EdgionConfigData`.  Applied / deleted during dynamic scenarios.
const REFERENCE_GRANT_YAML: &str = r#"apiVersion: gateway.networking.k8s.io/v1beta1
kind: ReferenceGrant
metadata:
  name: allow-edgion-test-ecd
  namespace: edgion-default
spec:
  from:
    - group: edgion.io
      kind: EdgionPlugins
      namespace: edgion-test
  to:
    - group: edgion.io
      kind: EdgionConfigData
"#;

pub struct EdgionConfigDataRefGrantTestSuite;

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Poll the configserver Admin API for an `EdgionPlugins` resource and return its
/// `status.conditions` array.  Retries up to `max_attempts` times with 500 ms delay.
async fn fetch_ep_conditions(ctx: &TestContext, name: &str, max_attempts: usize) -> Result<Vec<Value>, String> {
    let url = format!(
        "{}/configserver/EdgionPlugins?namespace={}&name={}",
        ctx.admin_api_url(),
        PLUGIN_NS,
        name
    );

    let mut last_err = format!("EdgionPlugins '{}' not found", name);

    for _ in 0..max_attempts {
        let mut req = ctx.http_client.get(&url);
        if let Some(ref token) = ctx.admin_token {
            req = req.header("Authorization", format!("Bearer {}", token));
        }

        match req.send().await {
            Ok(resp) if resp.status().is_success() => match resp.json::<Value>().await {
                Ok(body) if body["success"].as_bool().unwrap_or(false) => {
                    if let Some(conditions) = body["data"]["status"]["conditions"].as_array() {
                        if !conditions.is_empty() {
                            return Ok(conditions.clone());
                        }
                        last_err = format!("EdgionPlugins '{}' found but status.conditions is empty", name);
                    } else {
                        last_err = format!("EdgionPlugins '{}' found but status.conditions not present", name);
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

        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    Err(last_err)
}

/// Find the `ResolvedRefs` condition in a conditions array.
fn find_resolved_refs(conditions: &[Value]) -> Option<&Value> {
    conditions.iter().find(|c| c["type"].as_str() == Some("ResolvedRefs"))
}

/// Return `true` when the `ResolvedRefs` condition has `status=False` and
/// `reason=RefNotPermitted`.
fn is_ref_not_permitted(conditions: &[Value]) -> bool {
    find_resolved_refs(conditions)
        .is_some_and(|c| c["status"].as_str() == Some("False") && c["reason"].as_str() == Some("RefNotPermitted"))
}

/// Return `true` when the `ResolvedRefs` condition has `status=True`.
fn is_resolved_true(conditions: &[Value]) -> bool {
    find_resolved_refs(conditions).is_some_and(|c| c["status"].as_str() == Some("True"))
}

// ---------------------------------------------------------------------------
// Scenario implementations
// ---------------------------------------------------------------------------

impl EdgionConfigDataRefGrantTestSuite {
    // =========================================================================
    // Scenario 1: denyRefs cross-namespace without grant
    //
    // EdgionPlugins 'ecd-deny-refs-cross-ns' (ns=edgion-test) has a denyRefs entry
    // pointing to EdgionConfigData 'ecd-refgrant-blocklist' in edgion-default.
    // No ReferenceGrant is present.  Expected: ResolvedRefs=False(RefNotPermitted).
    //
    // Reason string asserted: "RefNotPermitted" (not the Rust enum variant).
    // =========================================================================

    fn test_deny_refs_no_grant_ref_not_permitted() -> TestCase {
        TestCase::new(
            "deny_refs_no_grant_ref_not_permitted",
            "Cross-ns denyRefs without ReferenceGrant → ResolvedRefs=False(RefNotPermitted)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    match fetch_ep_conditions(&ctx, "ecd-deny-refs-cross-ns", 20).await {
                        Err(e) => TestResult::failed(start.elapsed(), e),
                        Ok(conditions) => {
                            if is_ref_not_permitted(&conditions) {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "ResolvedRefs=False(RefNotPermitted) as expected".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected ResolvedRefs=False(RefNotPermitted), got: {:?}", conditions),
                                )
                            }
                        }
                    }
                })
            },
        )
    }

    // =========================================================================
    // Scenario 2: allowRefs cross-namespace without grant — RefNotPermitted
    //
    // EdgionPlugins 'ecd-allow-refs-cross-ns' (ns=edgion-test) carries a
    // whitelist-with-inline-deny config: an inline deny list (10.0.0.0/8) plus
    // an allowRefs cross-ns reference (no ReferenceGrant in edgion-default).
    // Expected: ResolvedRefs=False(RefNotPermitted) on the controller status.
    //
    // Data-plane deny-all fail-closed for a denied allowRef is verified by Task 7
    // gateway unit tests (HTTP DenyByWhitelistMiss; L4/TLS StreamPluginResult::Deny);
    // this integration scenario asserts the controller status (RefNotPermitted) only.
    // =========================================================================

    fn test_allow_refs_no_grant_ref_not_permitted() -> TestCase {
        TestCase::new(
            "allow_refs_no_grant_ref_not_permitted",
            "Cross-ns allowRefs without ReferenceGrant → ResolvedRefs=False(RefNotPermitted)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    match fetch_ep_conditions(&ctx, "ecd-allow-refs-cross-ns", 20).await {
                        Err(e) => TestResult::failed(start.elapsed(), e),
                        Ok(conditions) => {
                            if is_ref_not_permitted(&conditions) {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "ResolvedRefs=False(RefNotPermitted) as expected for denied allowRefs".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected ResolvedRefs=False(RefNotPermitted) for denied allowRefs, got: {:?}",
                                        conditions
                                    ),
                                )
                            }
                        }
                    }
                })
            },
        )
    }

    // =========================================================================
    // Scenario 3: visibility:Cluster bypasses grant requirement
    //
    // EdgionPlugins 'ecd-cluster-ref-cross-ns' (ns=edgion-test) references
    // EdgionConfigData 'ecd-refgrant-cluster-data' (ns=edgion-default) which has
    // visibility:Cluster.  No ReferenceGrant is needed.
    // Expected: ResolvedRefs=True.
    // =========================================================================

    fn test_visibility_cluster_bypasses_grant() -> TestCase {
        TestCase::new(
            "visibility_cluster_bypasses_grant",
            "Cross-ns ref to visibility:Cluster EdgionConfigData → ResolvedRefs=True without grant",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    match fetch_ep_conditions(&ctx, "ecd-cluster-ref-cross-ns", 20).await {
                        Err(e) => TestResult::failed(start.elapsed(), e),
                        Ok(conditions) => {
                            if is_resolved_true(&conditions) {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "ResolvedRefs=True: visibility:Cluster bypassed grant check".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected ResolvedRefs=True for visibility:Cluster target, got: {:?}",
                                        conditions
                                    ),
                                )
                            }
                        }
                    }
                })
            },
        )
    }

    // =========================================================================
    // Scenario 4: same-namespace ref always resolves
    //
    // EdgionPlugins 'ecd-same-ns-ref' (ns=edgion-test) references
    // EdgionConfigData 'ecd-same-ns-data' (ns=edgion-test) — same namespace.
    // Same-namespace refs are always permitted regardless of the validation flag
    // and without any ReferenceGrant.
    // Expected: ResolvedRefs=True.
    // =========================================================================

    fn test_same_ns_ref_always_resolves() -> TestCase {
        TestCase::new(
            "same_ns_ref_always_resolves",
            "Same-namespace EdgionConfigDataRef → ResolvedRefs=True (no grant needed)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    match fetch_ep_conditions(&ctx, "ecd-same-ns-ref", 20).await {
                        Err(e) => TestResult::failed(start.elapsed(), e),
                        Ok(conditions) => {
                            if is_resolved_true(&conditions) {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "ResolvedRefs=True: same-ns ref always resolves".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!("Expected ResolvedRefs=True for same-ns ref, got: {:?}", conditions),
                                )
                            }
                        }
                    }
                })
            },
        )
    }

    // =========================================================================
    // Scenario 5: add ReferenceGrant → ResolvedRefs transitions to True;
    //             delete grant → ResolvedRefs transitions back to False.
    //
    // Uses 'ecd-deny-refs-cross-ns' (same resource as Scenario 1).
    // The test applies the grant inline, polls for True, then deletes it and
    // polls for False again to validate the requeue logic in both directions.
    //
    // Note: this test modifies shared cluster state and restores it at the end,
    // so it must run after Scenarios 1-4.  If the grant already exists (e.g.
    // from a failed previous run), the delete+re-add cycle still exercises both
    // transitions.
    // =========================================================================

    fn test_add_grant_restores_resolved_refs() -> TestCase {
        TestCase::new(
            "add_grant_restores_resolved_refs",
            "Adding ReferenceGrant → ResolvedRefs=True; deleting it → ResolvedRefs=False(RefNotPermitted)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    let ep_name = "ecd-deny-refs-cross-ns";

                    // ── Pre-condition: ensure no grant is present ──────────
                    let _ = ctx
                        .delete_resource("ReferenceGrant", DATA_NS, "allow-edgion-test-ecd")
                        .await;

                    // Allow the controller to requeue after the delete.
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    // Verify RefNotPermitted is the starting state.
                    let initial_ok = {
                        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
                        let mut found = false;
                        while tokio::time::Instant::now() < deadline {
                            if let Ok(conds) = fetch_ep_conditions(&ctx, ep_name, 1).await {
                                if is_ref_not_permitted(&conds) {
                                    found = true;
                                    break;
                                }
                            }
                            tokio::time::sleep(Duration::from_millis(300)).await;
                        }
                        found
                    };

                    if !initial_ok {
                        return TestResult::failed(
                            start.elapsed(),
                            "Pre-condition failed: expected RefNotPermitted before adding grant".to_string(),
                        );
                    }

                    // ── Apply the ReferenceGrant ───────────────────────────
                    if let Err(e) = ctx.apply_yaml(REFERENCE_GRANT_YAML).await {
                        return TestResult::failed(start.elapsed(), format!("Failed to apply ReferenceGrant: {}", e));
                    }

                    // ── Poll until ResolvedRefs=True ───────────────────────
                    // FSEvents on macOS adds ~2 s latency; poll for up to 8 s.
                    let grant_deadline = tokio::time::Instant::now() + Duration::from_secs(8);
                    let mut grant_observed = false;
                    let mut last_conds: Option<Vec<Value>> = None;
                    while tokio::time::Instant::now() < grant_deadline {
                        match fetch_ep_conditions(&ctx, ep_name, 1).await {
                            Ok(conds) => {
                                if is_resolved_true(&conds) {
                                    grant_observed = true;
                                    break;
                                }
                                last_conds = Some(conds);
                            }
                            Err(_) => {}
                        }
                        tokio::time::sleep(Duration::from_millis(300)).await;
                    }

                    if !grant_observed {
                        // Attempt cleanup before failing.
                        let _ = ctx
                            .delete_resource("ReferenceGrant", DATA_NS, "allow-edgion-test-ecd")
                            .await;
                        return TestResult::failed(
                            start.elapsed(),
                            format!(
                                "After adding ReferenceGrant, expected ResolvedRefs=True; got: {:?}",
                                last_conds
                            ),
                        );
                    }

                    // ── Delete the grant and poll for RefNotPermitted ──────
                    if let Err(e) = ctx
                        .delete_resource("ReferenceGrant", DATA_NS, "allow-edgion-test-ecd")
                        .await
                    {
                        return TestResult::failed(start.elapsed(), format!("Failed to delete ReferenceGrant: {}", e));
                    }

                    let del_deadline = tokio::time::Instant::now() + Duration::from_secs(8);
                    let mut del_observed = false;
                    let mut last_del_conds: Option<Vec<Value>> = None;
                    while tokio::time::Instant::now() < del_deadline {
                        match fetch_ep_conditions(&ctx, ep_name, 1).await {
                            Ok(conds) => {
                                if is_ref_not_permitted(&conds) {
                                    del_observed = true;
                                    break;
                                }
                                last_del_conds = Some(conds);
                            }
                            Err(_) => {}
                        }
                        tokio::time::sleep(Duration::from_millis(300)).await;
                    }

                    if del_observed {
                        TestResult::passed_with_message(
                            start.elapsed(),
                            "Add grant → ResolvedRefs=True; delete grant → RefNotPermitted".to_string(),
                        )
                    } else {
                        TestResult::failed(
                            start.elapsed(),
                            format!(
                                "After deleting ReferenceGrant, expected RefNotPermitted; got: {:?}",
                                last_del_conds
                            ),
                        )
                    }
                })
            },
        )
    }

    // =========================================================================
    // Scenario I-2a: overrideRef cross-namespace without grant
    //
    // EdgionPlugins 'ecd-override-ref-cross-ns' (ns=edgion-test) has a RegionRoute
    // plugin with an overrideRef pointing to EdgionConfigData 'ecd-region-override'
    // in edgion-default.  No ReferenceGrant is present.
    // Expected: ResolvedRefs=False(RefNotPermitted) on the controller status.
    //
    // Data-plane fail-to-base behavior (base regions used when overrideRef is denied)
    // is verified by Task 7 gateway unit tests; this scenario asserts controller
    // status (RefNotPermitted) only.
    // =========================================================================

    fn test_override_ref_no_grant_ref_not_permitted() -> TestCase {
        TestCase::new(
            "override_ref_no_grant_ref_not_permitted",
            "Cross-ns RegionRoute overrideRef without ReferenceGrant → ResolvedRefs=False(RefNotPermitted)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    match fetch_ep_conditions(&ctx, "ecd-override-ref-cross-ns", 20).await {
                        Err(e) => TestResult::failed(start.elapsed(), e),
                        Ok(conditions) => {
                            if is_ref_not_permitted(&conditions) {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "ResolvedRefs=False(RefNotPermitted) as expected for denied overrideRef"
                                        .to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected ResolvedRefs=False(RefNotPermitted) for denied overrideRef, got: {:?}",
                                        conditions
                                    ),
                                )
                            }
                        }
                    }
                })
            },
        )
    }

    // =========================================================================
    // Scenario I-2b: activeProfileRef cross-namespace without grant
    //
    // EdgionPlugins 'ecd-active-profile-ref-cross-ns' (ns=edgion-test) has a
    // GlobalAccessControl plugin with an activeProfileRef pointing to
    // EdgionConfigData 'ecd-active-selector' (type: Selector) in edgion-default.
    // No ReferenceGrant is present.
    // Expected: ResolvedRefs=False(RefNotPermitted) on the controller status.
    //
    // Data-plane fail-to-base behavior (base active_profile used when
    // activeProfileRef is denied) is verified by Task 7 gateway unit tests;
    // this scenario asserts controller status (RefNotPermitted) only.
    // =========================================================================

    fn test_active_profile_ref_no_grant_ref_not_permitted() -> TestCase {
        TestCase::new(
            "active_profile_ref_no_grant_ref_not_permitted",
            "Cross-ns GlobalAccessControl activeProfileRef without ReferenceGrant → ResolvedRefs=False(RefNotPermitted)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    match fetch_ep_conditions(&ctx, "ecd-active-profile-ref-cross-ns", 20).await {
                        Err(e) => TestResult::failed(start.elapsed(), e),
                        Ok(conditions) => {
                            if is_ref_not_permitted(&conditions) {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "ResolvedRefs=False(RefNotPermitted) as expected for denied activeProfileRef".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected ResolvedRefs=False(RefNotPermitted) for denied activeProfileRef, got: {:?}",
                                        conditions
                                    ),
                                )
                            }
                        }
                    }
                })
            },
        )
    }

    // =========================================================================
    // Scenario I-3: missing EdgionConfigData target → InvalidEdgionConfigDataRef
    //
    // EdgionPlugins 'ecd-missing-ref' (ns=edgion-test) references a non-existent
    // EdgionConfigData name 'ecd-does-not-exist' in the same namespace (edgion-test).
    // Same-namespace refs need no ReferenceGrant; the failure here is a missing target,
    // not a permission denial.
    // Expected: ResolvedRefs=False, reason="InvalidEdgionConfigDataRef".
    //
    // This distinguishes the "target absent" path from "target present but cross-ns
    // without grant" (RefNotPermitted).
    // =========================================================================

    fn test_missing_ref_invalid_edgion_config_data_ref() -> TestCase {
        TestCase::new(
            "missing_ref_invalid_edgion_config_data_ref",
            "Same-ns allowRefs to non-existent EdgionConfigData → ResolvedRefs=False(InvalidEdgionConfigDataRef)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    match fetch_ep_conditions(&ctx, "ecd-missing-ref", 20).await {
                        Err(e) => TestResult::failed(start.elapsed(), e),
                        Ok(conditions) => {
                            let resolved = find_resolved_refs(&conditions);
                            let ok = resolved.is_some_and(|c| {
                                c["status"].as_str() == Some("False")
                                    && c["reason"].as_str() == Some("InvalidEdgionConfigDataRef")
                            });
                            if ok {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "ResolvedRefs=False(InvalidEdgionConfigDataRef) as expected for missing target"
                                        .to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected ResolvedRefs=False(InvalidEdgionConfigDataRef) for missing target, got: {:?}",
                                        conditions
                                    ),
                                )
                            }
                        }
                    }
                })
            },
        )
    }
}

// ---------------------------------------------------------------------------
// TestSuite impl
// ---------------------------------------------------------------------------

#[async_trait]
impl TestSuite for EdgionConfigDataRefGrantTestSuite {
    fn name(&self) -> &str {
        "Status/RefGrant/EdgionConfigData"
    }

    fn port_key(&self) -> Option<&str> {
        // Status-only suite: no gateway port needed.
        None
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            // Static assertions — no cluster mutation.
            Self::test_deny_refs_no_grant_ref_not_permitted(),
            Self::test_allow_refs_no_grant_ref_not_permitted(),
            Self::test_visibility_cluster_bypasses_grant(),
            Self::test_same_ns_ref_always_resolves(),
            // I-2: overrideRef / activeProfileRef cross-ns without grant.
            Self::test_override_ref_no_grant_ref_not_permitted(),
            Self::test_active_profile_ref_no_grant_ref_not_permitted(),
            // I-3: missing EdgionConfigData target.
            Self::test_missing_ref_invalid_edgion_config_data_ref(),
            // Dynamic assertion — adds and removes a ReferenceGrant.
            // Must run last to avoid interfering with the static assertions.
            Self::test_add_grant_restores_resolved_refs(),
        ]
    }
}
