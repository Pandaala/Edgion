//! Shared helpers for Controller status testing via Admin API

use crate::framework::TestContext;
use serde_json::Value;
use std::time::{Duration, Instant};

/// Fetch a namespaced resource from Controller Admin API (configserver endpoint)
pub async fn fetch_resource_status(
    ctx: &TestContext,
    kind: &str,
    namespace: &str,
    name: &str,
) -> Result<Value, String> {
    let url = format!(
        "{}/configserver/{}?namespace={}&name={}",
        ctx.admin_api_url(),
        kind,
        namespace,
        name
    );

    let mut req = ctx.http_client.get(&url);
    if let Some(ref token) = ctx.admin_token {
        req = req.header("Authorization", format!("Bearer {}", token));
    }
    let response = req
        .send()
        .await
        .map_err(|e| format!("Failed to fetch {}: {}", kind, e))?;

    if !response.status().is_success() {
        return Err(format!("API returned status: {}", response.status()));
    }

    let body: Value = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse response: {}", e))?;

    if !body["success"].as_bool().unwrap_or(false) {
        return Err(format!("API error: {}", body["error"].as_str().unwrap_or("unknown")));
    }

    Ok(body["data"].clone())
}

/// Poll a namespaced resource until `accept` matches the observed status.
///
/// The Controller writes status asynchronously, and a resource whose refs are
/// unresolved is reprocessed once its dependency store settles, so the first
/// generation a reader observes can still carry an empty ancestor list. Waiting
/// for the expected condition keeps the assertion meaningful under full-suite
/// load; the error carries the last observed status so a real regression still
/// reports what the Controller published.
pub async fn wait_resource_status<F>(
    ctx: &TestContext,
    kind: &str,
    namespace: &str,
    name: &str,
    timeout: Duration,
    accept: F,
) -> Result<Value, String>
where
    F: Fn(&Value) -> bool,
{
    let deadline = Instant::now() + timeout;
    loop {
        let observed = match fetch_resource_status(ctx, kind, namespace, name).await {
            Ok(resource) => {
                if accept(&resource) {
                    return Ok(resource);
                }
                format!(
                    "got status: {}",
                    serde_json::to_string_pretty(&resource["status"]).unwrap_or_default()
                )
            }
            Err(error) => error,
        };
        if Instant::now() >= deadline {
            return Err(observed);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// Fetch a cluster-scoped resource from Controller Admin API
pub async fn fetch_cluster_resource_status(ctx: &TestContext, kind: &str, name: &str) -> Result<Value, String> {
    let url = format!("{}/configserver/{}?name={}", ctx.admin_api_url(), kind, name);

    let mut req = ctx.http_client.get(&url);
    if let Some(ref token) = ctx.admin_token {
        req = req.header("Authorization", format!("Bearer {}", token));
    }
    let response = req
        .send()
        .await
        .map_err(|e| format!("Failed to fetch {}: {}", kind, e))?;

    if !response.status().is_success() {
        return Err(format!("API returned status: {}", response.status()));
    }

    let body: Value = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse response: {}", e))?;

    if !body["success"].as_bool().unwrap_or(false) {
        return Err(format!("API error: {}", body["error"].as_str().unwrap_or("unknown")));
    }

    Ok(body["data"].clone())
}

/// Check if a resource status has a specific condition with expected status value.
/// Works with the `status.conditions[]` (Gateway / GatewayClass) pattern as well
/// as the `status.parents[]` (Route) and `status.ancestors[]` (Policy) patterns.
pub fn has_condition(status: &Value, condition_type: &str, expected_status: &str) -> bool {
    // Top-level conditions (Gateway / GatewayClass pattern)
    if check_conditions(&status["status"], condition_type, expected_status) {
        return true;
    }
    // Try parents (Route pattern)
    if let Some(parents) = status["status"]["parents"].as_array() {
        for parent in parents {
            if check_conditions(parent, condition_type, expected_status) {
                return true;
            }
        }
    }
    // Try ancestors (Policy pattern - BackendTLSPolicy uses this)
    if let Some(ancestors) = status["status"]["ancestors"].as_array() {
        for ancestor in ancestors {
            if check_conditions(ancestor, condition_type, expected_status) {
                return true;
            }
        }
    }
    false
}

/// Check if a condition exists with matching type, status, and message substring
pub fn has_condition_with_message(
    status: &Value,
    condition_type: &str,
    expected_status: &str,
    message_contains: &str,
) -> bool {
    // Top-level conditions (Gateway / GatewayClass pattern)
    if check_conditions_with_message(&status["status"], condition_type, expected_status, message_contains) {
        return true;
    }
    // Try parents
    if let Some(parents) = status["status"]["parents"].as_array() {
        for parent in parents {
            if check_conditions_with_message(parent, condition_type, expected_status, message_contains) {
                return true;
            }
        }
    }
    // Try ancestors
    if let Some(ancestors) = status["status"]["ancestors"].as_array() {
        for ancestor in ancestors {
            if check_conditions_with_message(ancestor, condition_type, expected_status, message_contains) {
                return true;
            }
        }
    }
    false
}

fn check_conditions(parent_or_ancestor: &Value, condition_type: &str, expected_status: &str) -> bool {
    if let Some(conditions) = parent_or_ancestor["conditions"].as_array() {
        conditions
            .iter()
            .any(|c| c["type"].as_str() == Some(condition_type) && c["status"].as_str() == Some(expected_status))
    } else {
        false
    }
}

fn check_conditions_with_message(
    parent_or_ancestor: &Value,
    condition_type: &str,
    expected_status: &str,
    message_contains: &str,
) -> bool {
    if let Some(conditions) = parent_or_ancestor["conditions"].as_array() {
        conditions.iter().any(|c| {
            c["type"].as_str() == Some(condition_type)
                && c["status"].as_str() == Some(expected_status)
                && c["message"].as_str().is_some_and(|m| m.contains(message_contains))
        })
    } else {
        false
    }
}
