//! ReferenceGrant Status Test Suite
//!
//! Tests the status system for cross-namespace references:
//! 1. Cross-ns reference with ReferenceGrant → ResolvedRefs=True
//! 2. Cross-ns reference without ReferenceGrant → ResolvedRefs=False
//! 3. ReferenceGrant arriving later triggers requeue and status update
//! 4. Multiple parent_refs have independent status

use super::super::helpers;
use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use async_trait::async_trait;
use serde_json::Value;
use std::time::{Duration, Instant};

pub struct RefGrantStatusTestSuite;

const ROUTE_NAMESPACE: &str = "edgion-default";

const REFERENCE_GRANT_YAML: &str = r#"apiVersion: gateway.networking.k8s.io/v1
kind: ReferenceGrant
metadata:
  name: allow-app
  namespace: backend
spec:
  from:
    - group: gateway.networking.k8s.io
      kind: HTTPRoute
      namespace: edgion-default
  to:
    - group: ""
      kind: Service
"#;

impl RefGrantStatusTestSuite {
    /// Check if status has ResolvedRefs=False condition with specific reason
    fn has_resolved_refs_false_with_reason(status: &Value, reason: &str) -> bool {
        if let Some(parents) = status["status"]["parents"].as_array() {
            for parent in parents {
                if let Some(conditions) = parent["conditions"].as_array() {
                    for cond in conditions {
                        if cond["type"].as_str() == Some("ResolvedRefs")
                            && cond["status"].as_str() == Some("False")
                            && cond["reason"].as_str() == Some(reason)
                        {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }

    /// Test: Cross-namespace reference with ReferenceGrant should have ResolvedRefs=True
    fn test_cross_ns_allowed() -> TestCase {
        TestCase::new(
            "cross_ns_allowed",
            "Cross-ns reference with ReferenceGrant should have ResolvedRefs=True",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Wait a bit for status to be computed
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    match helpers::fetch_resource_status(&ctx, "HTTPRoute", ROUTE_NAMESPACE, "cross-ns-route").await {
                        Ok(route) => {
                            if helpers::has_condition(&route, "ResolvedRefs", "True") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "ResolvedRefs=True as expected".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected ResolvedRefs=True, got status: {}",
                                        serde_json::to_string_pretty(&route["status"]).unwrap_or_default()
                                    ),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Test: Cross-namespace reference without ReferenceGrant should have ResolvedRefs=False
    fn test_cross_ns_denied() -> TestCase {
        TestCase::new(
            "cross_ns_denied",
            "Cross-ns reference without ReferenceGrant should have ResolvedRefs=False",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Wait a bit for status to be computed
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    match helpers::fetch_resource_status(&ctx, "HTTPRoute", ROUTE_NAMESPACE, "cross-ns-denied").await {
                        Ok(route) => {
                            if Self::has_resolved_refs_false_with_reason(&route, "RefNotPermitted") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "ResolvedRefs=False(RefNotPermitted) as expected".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected ResolvedRefs=False(RefNotPermitted), got status: {}",
                                        serde_json::to_string_pretty(&route["status"]).unwrap_or_default()
                                    ),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Test: Route with multiple parent_refs should have independent status for each
    fn test_multi_parent() -> TestCase {
        TestCase::new(
            "multi_parent",
            "Route with multiple parent_refs should have status for each parent",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Wait a bit for status to be computed
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    match helpers::fetch_resource_status(&ctx, "HTTPRoute", ROUTE_NAMESPACE, "multi-parent").await {
                        Ok(route) => {
                            if let Some(parents) = route["status"]["parents"].as_array() {
                                if parents.len() >= 2 {
                                    // Check that each parent has conditions
                                    let all_have_conditions = parents
                                        .iter()
                                        .all(|p| p["conditions"].as_array().map(|c| !c.is_empty()).unwrap_or(false));

                                    if all_have_conditions {
                                        TestResult::passed_with_message(
                                            start.elapsed(),
                                            format!("Found {} parents with conditions", parents.len()),
                                        )
                                    } else {
                                        TestResult::failed(
                                            start.elapsed(),
                                            "Not all parents have conditions".to_string(),
                                        )
                                    }
                                } else {
                                    TestResult::failed(
                                        start.elapsed(),
                                        format!("Expected >= 2 parents, got {}", parents.len()),
                                    )
                                }
                            } else {
                                TestResult::failed(start.elapsed(), "No parents in status".to_string())
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Test: Status has Accepted condition
    fn test_has_accepted_condition() -> TestCase {
        TestCase::new(
            "has_accepted_condition",
            "Route status should have Accepted condition",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Wait a bit for status to be computed
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    match helpers::fetch_resource_status(&ctx, "HTTPRoute", ROUTE_NAMESPACE, "cross-ns-route").await {
                        Ok(route) => {
                            let has_accepted = if let Some(parents) = route["status"]["parents"].as_array() {
                                parents.iter().any(|p| {
                                    p["conditions"].as_array().is_some_and(|conds| {
                                        conds.iter().any(|c| c["type"].as_str() == Some("Accepted"))
                                    })
                                })
                            } else {
                                false
                            };

                            if has_accepted {
                                TestResult::passed_with_message(start.elapsed(), "Found Accepted condition".to_string())
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected Accepted condition, got status: {}",
                                        serde_json::to_string_pretty(&route["status"]).unwrap_or_default()
                                    ),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Test: GatewayClass has Accepted=True condition
    fn test_gateway_class_accepted() -> TestCase {
        TestCase::new(
            "gateway_class_accepted",
            "GatewayClass status should have Accepted=True condition",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    tokio::time::sleep(Duration::from_millis(500)).await;

                    match helpers::fetch_cluster_resource_status(&ctx, "GatewayClass", "public-gateway").await {
                        Ok(gc) => {
                            let accepted_true = gc["status"]["conditions"].as_array().is_some_and(|conds| {
                                conds.iter().any(|c| {
                                    c["type"].as_str() == Some("Accepted") && c["status"].as_str() == Some("True")
                                })
                            });

                            if accepted_true {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "GatewayClass has Accepted=True".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected GatewayClass Accepted=True, got status: {}",
                                        serde_json::to_string_pretty(&gc["status"]).unwrap_or_default()
                                    ),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Test: HTTPRoute with invalid backend kind should have ResolvedRefs=False/InvalidKind
    fn test_invalid_backend_kind() -> TestCase {
        TestCase::new(
            "invalid_backend_kind",
            "HTTPRoute with kind=Foo backend should have ResolvedRefs=False(InvalidKind)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    match helpers::fetch_resource_status(&ctx, "HTTPRoute", ROUTE_NAMESPACE, "invalid-kind-route").await
                    {
                        Ok(route) => {
                            if Self::has_resolved_refs_false_with_reason(&route, "InvalidKind") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "ResolvedRefs=False(InvalidKind) as expected".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected ResolvedRefs=False(InvalidKind), got status: {}",
                                        serde_json::to_string_pretty(&route["status"]).unwrap_or_default()
                                    ),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Test: HTTPRoute with non-existent backend should have ResolvedRefs=False/BackendNotFound
    fn test_nonexistent_backend() -> TestCase {
        TestCase::new(
            "nonexistent_backend",
            "HTTPRoute with non-existent Service should have ResolvedRefs=False(BackendNotFound)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    match helpers::fetch_resource_status(&ctx, "HTTPRoute", ROUTE_NAMESPACE, "nonexistent-backend")
                        .await
                    {
                        Ok(route) => {
                            if Self::has_resolved_refs_false_with_reason(&route, "BackendNotFound") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "ResolvedRefs=False(BackendNotFound) as expected".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected ResolvedRefs=False(BackendNotFound), got status: {}",
                                        serde_json::to_string_pretty(&route["status"]).unwrap_or_default()
                                    ),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Test: HTTPRoute with one valid rule and one rule with ref-denied cross-ns backend
    /// should have Accepted=True and ResolvedRefs=False(RefNotPermitted).
    /// Per Gateway API conformance spec (httproute-partially-invalid-via-invalid-reference-grant),
    /// PartiallyInvalid is NOT required for backend ref issues — only ResolvedRefs is affected.
    fn test_partially_invalid() -> TestCase {
        TestCase::new(
            "partially_invalid",
            "HTTPRoute with one valid and one ref-denied rule: Accepted=True, ResolvedRefs=False(RefNotPermitted)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    match helpers::fetch_resource_status(
                        &ctx,
                        "HTTPRoute",
                        ROUTE_NAMESPACE,
                        "refgrant-partially-invalid",
                    )
                    .await
                    {
                        Ok(route) => {
                            // Accepted should be True (ref_denied does not make route unaccepted)
                            if !helpers::has_condition(&route, "Accepted", "True") {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected Accepted=True, got status: {}",
                                        serde_json::to_string_pretty(&route["status"]).unwrap_or_default()
                                    ),
                                );
                            }

                            // ResolvedRefs should be False(RefNotPermitted) — per Gateway API spec
                            if !Self::has_resolved_refs_false_with_reason(&route, "RefNotPermitted") {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected ResolvedRefs=False(RefNotPermitted), got status: {}",
                                        serde_json::to_string_pretty(&route["status"]).unwrap_or_default()
                                    ),
                                );
                            }

                            TestResult::passed_with_message(
                                start.elapsed(),
                                "Accepted=True, ResolvedRefs=False(RefNotPermitted) — as expected".to_string(),
                            )
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    // ── Gateway listener certificateRef cross-namespace tests (D1–D4) ──

    /// Helper: find a listener condition in Gateway status
    fn find_listener_condition<'a>(gateway: &'a Value, listener_name: &str, cond_type: &str) -> Option<&'a Value> {
        gateway["status"]["listeners"].as_array()?.iter().find_map(|l| {
            if l["name"].as_str() == Some(listener_name) {
                l["conditions"]
                    .as_array()?
                    .iter()
                    .find(|c| c["type"].as_str() == Some(cond_type))
            } else {
                None
            }
        })
    }

    /// D1: Gateway HTTPS listener cert ref cross-ns, wildcard ReferenceGrant (no name restriction)
    /// Uses cert-store namespace with a grant that allows ALL Secrets
    /// Expected: listener ResolvedRefs=True
    fn test_gateway_cert_refgrant_allowed() -> TestCase {
        TestCase::new(
            "gateway_cert_refgrant_allowed",
            "Gateway cross-ns cert ref with wildcard ReferenceGrant (all Secrets) should have listener ResolvedRefs=True",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    match helpers::fetch_resource_status(&ctx, "Gateway", "edgion-default", "cert-refgrant-all").await {
                        Ok(gw) => {
                            if let Some(cond) = Self::find_listener_condition(&gw, "https-grant-all", "ResolvedRefs") {
                                if cond["status"].as_str() == Some("True") {
                                    TestResult::passed_with_message(
                                        start.elapsed(),
                                        "listener ResolvedRefs=True as expected".to_string(),
                                    )
                                } else {
                                    TestResult::failed(
                                        start.elapsed(),
                                        format!(
                                            "Expected listener ResolvedRefs=True, got: {}",
                                            serde_json::to_string_pretty(&cond).unwrap_or_default()
                                        ),
                                    )
                                }
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "ResolvedRefs condition not found on listener 'https-grant-all', status: {}",
                                        serde_json::to_string_pretty(&gw["status"]).unwrap_or_default()
                                    ),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// D2: Gateway with two HTTPS listeners, ReferenceGrant allows only specific Secret name
    /// Listener referencing the allowed name → ResolvedRefs=True
    /// Listener referencing a different name → ResolvedRefs=False/RefNotPermitted
    fn test_gateway_cert_refgrant_specific_name() -> TestCase {
        TestCase::new(
            "gateway_cert_refgrant_specific_name",
            "Gateway cert ref: matching name→ResolvedRefs=True, mismatched name→ResolvedRefs=False(RefNotPermitted)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    match helpers::fetch_resource_status(&ctx, "Gateway", "edgion-default", "cert-refgrant-specific")
                        .await
                    {
                        Ok(gw) => {
                            // Check matching listener
                            let match_ok = if let Some(cond) =
                                Self::find_listener_condition(&gw, "https-name-match", "ResolvedRefs")
                            {
                                cond["status"].as_str() == Some("True")
                            } else {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "ResolvedRefs not found on listener 'https-name-match', status: {}",
                                        serde_json::to_string_pretty(&gw["status"]).unwrap_or_default()
                                    ),
                                );
                            };

                            if !match_ok {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected listener 'https-name-match' ResolvedRefs=True, status: {}",
                                        serde_json::to_string_pretty(&gw["status"]).unwrap_or_default()
                                    ),
                                );
                            }

                            // Check mismatched listener
                            let mismatch_ok = if let Some(cond) =
                                Self::find_listener_condition(&gw, "https-name-mismatch", "ResolvedRefs")
                            {
                                cond["status"].as_str() == Some("False")
                                    && cond["reason"].as_str() == Some("RefNotPermitted")
                            } else {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "ResolvedRefs not found on listener 'https-name-mismatch', status: {}",
                                        serde_json::to_string_pretty(&gw["status"]).unwrap_or_default()
                                    ),
                                );
                            };

                            if mismatch_ok {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "match→True, mismatch→False(RefNotPermitted) as expected".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected listener 'https-name-mismatch' ResolvedRefs=False(RefNotPermitted), status: {}",
                                        serde_json::to_string_pretty(&gw["status"]).unwrap_or_default()
                                    ),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// D3: Gateway cross-ns cert ref, ReferenceGrant exists but from.namespace does not match
    /// Expected: listener ResolvedRefs=False/RefNotPermitted
    fn test_gateway_cert_refgrant_invalid_from() -> TestCase {
        TestCase::new(
            "gateway_cert_refgrant_invalid_from",
            "Gateway cross-ns cert ref with ReferenceGrant from.namespace mismatch should have ResolvedRefs=False(RefNotPermitted)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    match helpers::fetch_resource_status(&ctx, "Gateway", "edgion-default", "cert-refgrant-invalid-from").await {
                        Ok(gw) => {
                            if let Some(cond) = Self::find_listener_condition(&gw, "https-invalid-from", "ResolvedRefs") {
                                if cond["status"].as_str() == Some("False")
                                    && cond["reason"].as_str() == Some("RefNotPermitted")
                                {
                                    TestResult::passed_with_message(
                                        start.elapsed(),
                                        "listener ResolvedRefs=False(RefNotPermitted) as expected".to_string(),
                                    )
                                } else {
                                    TestResult::failed(
                                        start.elapsed(),
                                        format!(
                                            "Expected ResolvedRefs=False(RefNotPermitted), got: {}",
                                            serde_json::to_string_pretty(&cond).unwrap_or_default()
                                        ),
                                    )
                                }
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "ResolvedRefs condition not found on listener 'https-invalid-from', status: {}",
                                        serde_json::to_string_pretty(&gw["status"]).unwrap_or_default()
                                    ),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// D4: Gateway cross-ns cert ref with no ReferenceGrant for Gateway kind at all
    /// Expected: listener ResolvedRefs=False/RefNotPermitted
    fn test_gateway_cert_no_refgrant() -> TestCase {
        TestCase::new(
            "gateway_cert_no_refgrant",
            "Gateway cross-ns cert ref with no ReferenceGrant should have ResolvedRefs=False(RefNotPermitted)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    match helpers::fetch_resource_status(&ctx, "Gateway", "edgion-default", "cert-refgrant-no-grant")
                        .await
                    {
                        Ok(gw) => {
                            if let Some(cond) = Self::find_listener_condition(&gw, "https-no-grant", "ResolvedRefs") {
                                if cond["status"].as_str() == Some("False")
                                    && cond["reason"].as_str() == Some("RefNotPermitted")
                                {
                                    TestResult::passed_with_message(
                                        start.elapsed(),
                                        "listener ResolvedRefs=False(RefNotPermitted) as expected".to_string(),
                                    )
                                } else {
                                    TestResult::failed(
                                        start.elapsed(),
                                        format!(
                                            "Expected ResolvedRefs=False(RefNotPermitted), got: {}",
                                            serde_json::to_string_pretty(&cond).unwrap_or_default()
                                        ),
                                    )
                                }
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "ResolvedRefs condition not found on listener 'https-no-grant', status: {}",
                                        serde_json::to_string_pretty(&gw["status"]).unwrap_or_default()
                                    ),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), e),
                    }
                })
            },
        )
    }

    /// Test: Deleting a ReferenceGrant should cause the route to become ResolvedRefs=False
    /// while keeping Accepted=True (route stays in route table, Gateway returns 500).
    fn test_delete_reference_grant() -> TestCase {
        TestCase::new(
            "delete_reference_grant",
            "Deleting ReferenceGrant: route keeps Accepted=True, ResolvedRefs becomes False, then restores",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();
                    tokio::time::sleep(Duration::from_millis(500)).await;

                    // Step 1: Verify initial state — cross-ns-route has ResolvedRefs=True
                    match helpers::fetch_resource_status(&ctx, "HTTPRoute", ROUTE_NAMESPACE, "cross-ns-route").await {
                        Ok(route) => {
                            if !helpers::has_condition(&route, "ResolvedRefs", "True") {
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Pre-condition failed: expected ResolvedRefs=True before deleting grant, got: {}",
                                        serde_json::to_string_pretty(&route["status"]).unwrap_or_default()
                                    ),
                                );
                            }
                        }
                        Err(e) => return TestResult::failed(start.elapsed(), format!("Pre-check failed: {}", e)),
                    }

                    // Step 2: Delete the ReferenceGrant
                    if let Err(e) = ctx.delete_resource("ReferenceGrant", "backend", "allow-app").await {
                        return TestResult::failed(start.elapsed(), format!("Failed to delete ReferenceGrant: {}", e));
                    }

                    // Step 3: Wait for revalidation and check ResolvedRefs=False + Accepted=True
                    tokio::time::sleep(Duration::from_secs(2)).await;

                    match helpers::fetch_resource_status(&ctx, "HTTPRoute", ROUTE_NAMESPACE, "cross-ns-route").await {
                        Ok(route) => {
                            if !helpers::has_condition(&route, "Accepted", "True") {
                                // Restore before failing
                                let _ = ctx.apply_yaml(REFERENCE_GRANT_YAML).await;
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "After deleting ReferenceGrant, Accepted should remain True, got: {}",
                                        serde_json::to_string_pretty(&route["status"]).unwrap_or_default()
                                    ),
                                );
                            }
                            if !Self::has_resolved_refs_false_with_reason(&route, "RefNotPermitted") {
                                // Restore before failing
                                let _ = ctx.apply_yaml(REFERENCE_GRANT_YAML).await;
                                return TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "After deleting ReferenceGrant, expected ResolvedRefs=False(RefNotPermitted), got: {}",
                                        serde_json::to_string_pretty(&route["status"]).unwrap_or_default()
                                    ),
                                );
                            }
                        }
                        Err(e) => {
                            let _ = ctx.apply_yaml(REFERENCE_GRANT_YAML).await;
                            return TestResult::failed(start.elapsed(), format!("Fetch after delete failed: {}", e));
                        }
                    }

                    // Step 4: Restore the ReferenceGrant
                    if let Err(e) = ctx.apply_yaml(REFERENCE_GRANT_YAML).await {
                        return TestResult::failed(start.elapsed(), format!("Failed to restore ReferenceGrant: {}", e));
                    }

                    // Step 5: Wait and verify ResolvedRefs goes back to True
                    tokio::time::sleep(Duration::from_secs(2)).await;

                    match helpers::fetch_resource_status(&ctx, "HTTPRoute", ROUTE_NAMESPACE, "cross-ns-route").await {
                        Ok(route) => {
                            if helpers::has_condition(&route, "ResolvedRefs", "True") {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "Delete→ResolvedRefs=False(RefNotPermitted)+Accepted=True, Restore→ResolvedRefs=True".to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "After restoring ReferenceGrant, expected ResolvedRefs=True, got: {}",
                                        serde_json::to_string_pretty(&route["status"]).unwrap_or_default()
                                    ),
                                )
                            }
                        }
                        Err(e) => TestResult::failed(start.elapsed(), format!("Fetch after restore failed: {}", e)),
                    }
                })
            },
        )
    }
}

#[async_trait]
impl TestSuite for RefGrantStatusTestSuite {
    fn name(&self) -> &str {
        "ReferenceGrant Status"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_gateway_class_accepted(),
            Self::test_has_accepted_condition(),
            Self::test_cross_ns_allowed(),
            Self::test_cross_ns_denied(),
            Self::test_multi_parent(),
            Self::test_invalid_backend_kind(),
            Self::test_nonexistent_backend(),
            Self::test_partially_invalid(),
            Self::test_gateway_cert_refgrant_allowed(),
            Self::test_gateway_cert_refgrant_specific_name(),
            Self::test_gateway_cert_refgrant_invalid_from(),
            Self::test_gateway_cert_no_refgrant(),
            Self::test_delete_reference_grant(),
        ]
    }
}
