// Port Conflict Detection Test Suite
//
// Tests Gateway API port conflict detection feature.
// Conflict detection is Gateway-internal only (not cross-Gateway).
// Validates that:
// 1. Same Gateway, same port listeners -> Conflicted=True, ListenersNotValid
// 2. Different Gateways, same port -> NO conflict (each Gateway healthy)
// 3. Same port with different hostnames (HTTP) -> NOT a conflict
//
// Required config files (in edgion-tests/integration/conf/Gateway/PortConflict/):
// - Gateway_internal_conflict.yaml     # Single Gateway with internal port conflict
// - Gateway_cross_conflict_A.yaml      # First Gateway, same port as B (no conflict)
// - Gateway_cross_conflict_B.yaml      # Second Gateway, same port as A (no conflict)
// - Gateway_same_port_diff_hostname.yaml # Same port, different hostnames (no conflict)

use crate::framework::{TestCase, TestContext, TestResult, TestSuite};
use serde::Deserialize;
use std::time::Instant;

fn is_k8s_mode() -> bool {
    matches!(
        std::env::var("EDGION_TEST_K8S_MODE")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes"
    )
}

/// Gateway status response structure from Admin API
#[derive(Debug, Deserialize)]
struct GatewayListResponse {
    data: Vec<Gateway>,
}

#[derive(Debug, Deserialize)]
struct Gateway {
    metadata: GatewayMetadata,
    status: Option<GatewayStatus>,
}

#[derive(Debug, Deserialize)]
struct GatewayMetadata {
    name: Option<String>,
    namespace: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GatewayStatus {
    conditions: Option<Vec<Condition>>,
    listeners: Option<Vec<ListenerStatus>>,
}

#[derive(Debug, Deserialize)]
struct ListenerStatus {
    name: String,
    conditions: Vec<Condition>,
}

#[derive(Debug, Deserialize)]
struct Condition {
    #[serde(rename = "type")]
    type_: String,
    status: String,
    reason: Option<String>,
    message: Option<String>,
}

/// Helper to check if a listener has Conflicted=True condition
fn is_listener_conflicted(listener_status: &ListenerStatus) -> bool {
    listener_status
        .conditions
        .iter()
        .any(|c| c.type_ == "Conflicted" && c.status == "True")
}

/// Helper to check if Gateway has ListenersNotValid condition
fn has_listeners_not_valid(status: &GatewayStatus) -> bool {
    status
        .conditions
        .as_ref()
        .map(|conditions| {
            conditions
                .iter()
                .any(|c| c.type_ == "ListenersNotValid" && c.status == "True")
        })
        .unwrap_or(false)
}

/// Helper to find a Gateway by name from the response
fn find_gateway<'a>(gateways: &'a [Gateway], name: &str) -> Option<&'a Gateway> {
    gateways.iter().find(|g| g.metadata.name.as_deref() == Some(name))
}

/// Helper to find a Listener status by name
fn find_listener_status<'a>(status: &'a GatewayStatus, listener_name: &str) -> Option<&'a ListenerStatus> {
    status.listeners.as_ref()?.iter().find(|ls| ls.name == listener_name)
}

/// Require an observed no-conflict verdict; missing status is not evidence.
fn require_listener_not_conflicted(status: &GatewayStatus, name: &str) -> Result<(), String> {
    let mut listeners = status
        .listeners
        .iter()
        .flatten()
        .filter(|listener| listener.name == name);
    let listener = listeners
        .next()
        .ok_or_else(|| format!("Missing Listener status: {name}"))?;
    if listeners.next().is_some() {
        return Err(format!("Duplicate Listener status: {name}"));
    }
    let mut conditions = listener
        .conditions
        .iter()
        .filter(|condition| condition.type_ == "Conflicted");
    let condition = conditions
        .next()
        .ok_or_else(|| format!("Missing Conflicted condition: {name}"))?;
    if conditions.next().is_some() || condition.status != "False" {
        return Err(format!("Expected one Conflicted=False condition: {name}"));
    }
    Ok(())
}

pub struct PortConflictTestSuite;

impl PortConflictTestSuite {
    /// Test: Single Gateway internal port conflict
    fn test_internal_conflict() -> TestCase {
        TestCase::new(
            "internal_port_conflict",
            "Test internal port conflict (same Gateway, two Listeners on same port)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Fetch all Gateways from Admin API
                    let admin_url = format!("{}/configserver/Gateway/list", ctx.admin_api_url());
                    let mut req = ctx.http_client.get(&admin_url);
                    if let Some(ref token) = ctx.admin_token {
                        req = req.header("Authorization", format!("Bearer {}", token));
                    }
                    let response = match req.send().await {
                        Ok(resp) => resp,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Failed to fetch Gateways: {}", e))
                        }
                    };

                    let gateways: GatewayListResponse = match response.json().await {
                        Ok(data) => data,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Failed to parse response: {}", e))
                        }
                    };

                    // Find the internal conflict Gateway
                    let Some(gateway) = find_gateway(&gateways.data, "port-conflict-internal") else {
                        if is_k8s_mode() {
                            return TestResult::passed_with_message(
                                start.elapsed(),
                                "K8s admission rejected internal duplicate listener manifest; treated as expected"
                                    .to_string(),
                            );
                        }
                        return TestResult::failed(
                            start.elapsed(),
                            "Gateway 'port-conflict-internal' not found".to_string(),
                        );
                    };

                    let Some(status) = &gateway.status else {
                        return TestResult::failed(start.elapsed(), "Gateway has no status".to_string());
                    };

                    // Check that Gateway has ListenersNotValid condition
                    if !has_listeners_not_valid(status) {
                        return TestResult::failed(
                            start.elapsed(),
                            "Gateway should have ListenersNotValid condition".to_string(),
                        );
                    }

                    // Check that both Listeners are marked as Conflicted
                    let http1 = find_listener_status(status, "http-1");
                    let http2 = find_listener_status(status, "http-2");

                    match (http1, http2) {
                        (Some(l1), Some(l2)) => {
                            let l1_conflicted = is_listener_conflicted(l1);
                            let l2_conflicted = is_listener_conflicted(l2);

                            if l1_conflicted && l2_conflicted {
                                TestResult::passed_with_message(
                                    start.elapsed(),
                                    "✓ Both Listeners marked as Conflicted=True, Gateway has ListenersNotValid"
                                        .to_string(),
                                )
                            } else {
                                TestResult::failed(
                                    start.elapsed(),
                                    format!(
                                        "Expected both Listeners Conflicted=True, got http-1={}, http-2={}",
                                        l1_conflicted, l2_conflicted
                                    ),
                                )
                            }
                        }
                        _ => TestResult::failed(start.elapsed(), "Failed to find Listener statuses".to_string()),
                    }
                })
            },
        )
    }

    /// Test: Cross-Gateway same port is NOT a conflict.
    /// Conflict detection is Gateway-internal only, so two different
    /// Gateways using the same port should each be healthy (no Conflicted, no ListenersNotValid).
    fn test_cross_gateway_no_conflict() -> TestCase {
        TestCase::new(
            "cross_gateway_no_conflict",
            "Test cross-Gateway same port is NOT a conflict (conflict is Gateway-internal only)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Fetch all Gateways from Admin API
                    let admin_url = format!("{}/configserver/Gateway/list", ctx.admin_api_url());
                    let mut req = ctx.http_client.get(&admin_url);
                    if let Some(ref token) = ctx.admin_token {
                        req = req.header("Authorization", format!("Bearer {}", token));
                    }
                    let response = match req.send().await {
                        Ok(resp) => resp,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Failed to fetch Gateways: {}", e))
                        }
                    };

                    let gateways: GatewayListResponse = match response.json().await {
                        Ok(data) => data,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Failed to parse response: {}", e))
                        }
                    };

                    // Find both Gateways
                    let gateway_a = gateways.data.iter().find(|gateway| {
                        gateway.metadata.namespace.as_deref() == Some("edgion-test")
                            && gateway.metadata.name.as_deref() == Some("port-conflict-cross-a")
                    });
                    let gateway_b = gateways.data.iter().find(|gateway| {
                        gateway.metadata.namespace.as_deref() == Some("edgion-test")
                            && gateway.metadata.name.as_deref() == Some("port-conflict-cross-b")
                    });

                    match (gateway_a, gateway_b) {
                        (Some(ga), Some(gb)) => {
                            let status_a = ga.status.as_ref();
                            let status_b = gb.status.as_ref();

                            if status_a.is_none() || status_b.is_none() {
                                return TestResult::failed(
                                    start.elapsed(),
                                    "One or both Gateways have no status".to_string(),
                                );
                            }

                            let status_a = status_a.unwrap();
                            let status_b = status_b.unwrap();

                            // Neither Gateway should have ListenersNotValid
                            let a_invalid = has_listeners_not_valid(status_a);
                            let b_invalid = has_listeners_not_valid(status_b);

                            for (name, status) in
                                [("port-conflict-cross-a", status_a), ("port-conflict-cross-b", status_b)]
                            {
                                if let Err(error) = require_listener_not_conflicted(status, "http") {
                                    return TestResult::failed(start.elapsed(), format!("{name}: {error}"));
                                }
                            }
                            if a_invalid || b_invalid {
                                return TestResult::failed(
                                    start.elapsed(),
                                    "Unexpected ListenersNotValid=True for cross-Gateway listeners".to_string(),
                                );
                            }
                            TestResult::passed_with_message(
                                start.elapsed(),
                                "Both Gateways report http Conflicted=False (cross-Gateway same port is allowed)"
                                    .to_string(),
                            )
                        }
                        _ => TestResult::failed(
                            start.elapsed(),
                            "Failed to find one or both cross-Gateway Gateways".to_string(),
                        ),
                    }
                })
            },
        )
    }

    /// Test: Same port with different hostnames (no conflict for HTTP)
    fn test_no_conflict_diff_hostname() -> TestCase {
        TestCase::new(
            "no_conflict_different_hostname",
            "Test same port with different hostnames (should NOT be a conflict for HTTP)",
            |ctx: TestContext| {
                Box::pin(async move {
                    let start = Instant::now();

                    // Fetch all Gateways from Admin API
                    let admin_url = format!("{}/configserver/Gateway/list", ctx.admin_api_url());
                    let mut req = ctx.http_client.get(&admin_url);
                    if let Some(ref token) = ctx.admin_token {
                        req = req.header("Authorization", format!("Bearer {}", token));
                    }
                    let response = match req.send().await {
                        Ok(resp) => resp,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Failed to fetch Gateways: {}", e))
                        }
                    };

                    let gateways: GatewayListResponse = match response.json().await {
                        Ok(data) => data,
                        Err(e) => {
                            return TestResult::failed(start.elapsed(), format!("Failed to parse response: {}", e))
                        }
                    };

                    // Find the no-conflict Gateway
                    let Some(gateway) = gateways.data.iter().find(|gateway| {
                        gateway.metadata.namespace.as_deref() == Some("edgion-test")
                            && gateway.metadata.name.as_deref() == Some("port-no-conflict-hostname")
                    }) else {
                        return TestResult::failed(
                            start.elapsed(),
                            "Gateway 'port-no-conflict-hostname' not found".to_string(),
                        );
                    };

                    let Some(status) = &gateway.status else {
                        return TestResult::failed(start.elapsed(), "Gateway has no status".to_string());
                    };

                    // Check that Gateway does NOT have ListenersNotValid condition
                    if has_listeners_not_valid(status) {
                        return TestResult::failed(
                            start.elapsed(),
                            "Gateway should NOT have ListenersNotValid (different hostnames)".to_string(),
                        );
                    }

                    for name in ["api", "web"] {
                        if let Err(error) = require_listener_not_conflicted(status, name) {
                            return TestResult::failed(start.elapsed(), error);
                        }
                    }
                    TestResult::passed_with_message(
                        start.elapsed(),
                        "Same port, different hostnames: both Listeners report Conflicted=False".to_string(),
                    )
                })
            },
        )
    }
}

impl TestSuite for PortConflictTestSuite {
    fn name(&self) -> &str {
        "Port Conflict Detection Tests"
    }

    fn test_cases(&self) -> Vec<TestCase> {
        vec![
            Self::test_internal_conflict(),
            Self::test_cross_gateway_no_conflict(),
            Self::test_no_conflict_diff_hostname(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn no_conflict_requires_an_explicit_named_verdict() {
        for document in [
            json!({}),
            json!({"listeners": []}),
            json!({"listeners": [{"name": "other", "conditions": [{"type": "Conflicted", "status": "False"}]}]}),
            json!({"listeners": [{"name": "api", "conditions": []}]}),
            json!({"listeners": [{"name": "api", "conditions": [{"type": "Conflicted", "status": "Unknown"}]}]}),
            json!({"listeners": [{"name": "api", "conditions": [{"type": "Conflicted", "status": "True"}]}]}),
            json!({"listeners": [{"name": "api", "conditions": [{"type": "Conflicted", "status": "False"}, {"type": "Conflicted", "status": "True"}]}]}),
        ] {
            let status: GatewayStatus = serde_json::from_value(document).unwrap();
            assert!(require_listener_not_conflicted(&status, "api").is_err());
        }
        let mut status: GatewayStatus = serde_json::from_value(json!({"listeners": [
            {"name": "api", "conditions": [{"type": "Conflicted", "status": "False"}]},
            {"name": "web", "conditions": [{"type": "Conflicted", "status": "False"}]}
        ]}))
        .unwrap();
        assert!(!has_listeners_not_valid(&status));
        for name in ["api", "web"] {
            assert!(require_listener_not_conflicted(&status, name).is_ok());
        }
        status.conditions = Some(vec![Condition {
            type_: "ListenersNotValid".into(),
            status: "True".into(),
            reason: None,
            message: None,
        }]);
        assert!(has_listeners_not_valid(&status));
    }
}
