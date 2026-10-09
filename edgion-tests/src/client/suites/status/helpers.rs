//! Shared helpers for Controller status testing via Admin API

use crate::framework::TestContext;
use serde_json::Value;
use std::time::{Duration, Instant};

/// Select this Controller's local status from the Kubernetes envelope, or
/// return the native status used by filesystem and etcd integration tests.
pub fn controller_local_status<'a>(status: &'a Value, controller_name: &str) -> Option<&'a Value> {
    match status["controllers"].as_array() {
        Some(controllers) => controllers
            .iter()
            .find(|entry| entry["controllerName"] == controller_name)
            .map(|entry| &entry["status"]),
        None => Some(status),
    }
}

#[cfg(test)]
mod controller_status_tests {
    use super::controller_local_status;
    use serde_json::json;

    #[test]
    fn selects_only_the_matching_controller_and_preserves_native_status() {
        let native = json!({"conditions": [{"type": "Accepted", "status": "True"}]});
        assert_eq!(
            controller_local_status(&native, "edgion.io/gateway-controller"),
            Some(&native)
        );

        let partitioned = json!({"controllers": [
            {"controllerName": "foreign.example/controller", "status": {"conditions": []}},
            {"controllerName": "edgion.io/gateway-controller", "status": native}
        ]});
        assert_eq!(
            controller_local_status(&partitioned, "edgion.io/gateway-controller"),
            Some(&partitioned["controllers"][1]["status"])
        );
        assert_eq!(
            controller_local_status(&partitioned, "missing.example/controller"),
            None
        );
    }
}

/// Route namespace used by the DeniedBackendRef fixtures in the selected topology.
/// The local fixtures use edgion-test -> backend, while Kubernetes fixtures use
/// edgion-default -> edgion-backend. Never probe another namespace on a 404: a
/// missing fixture must remain a failed precondition, not a skipped assertion.
pub fn l4_denied_backend_ref_namespace() -> &'static str {
    let k8s = std::env::var("EDGION_TEST_K8S_MODE")
        .is_ok_and(|value| matches!(value.to_ascii_lowercase().as_str(), "true" | "1" | "yes"));
    denied_backend_ref_namespace_for(k8s)
}

fn denied_backend_ref_namespace_for(k8s: bool) -> &'static str {
    if k8s {
        "edgion-default"
    } else {
        "edgion-test"
    }
}

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

#[cfg(test)]
mod tests {
    use super::denied_backend_ref_namespace_for;

    #[test]
    fn denied_backend_ref_namespaces_match_both_fixture_trees() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        for (tree, k8s, backend_namespace) in [("integration", false, "backend"), ("k8s", true, "edgion-backend")] {
            for route in [
                "TCPRoute/DeniedBackendRef/04_TCPRoute.yaml",
                "UDPRoute/DeniedBackendRef/04_UDPRoute.yaml",
                "TLSRoute/DeniedBackendRef/07_TLSRoute_terminate.yaml",
                "TLSRoute/DeniedBackendRef/08_TLSRoute_passthrough.yaml",
            ] {
                let path = root.join(tree).join("conf").join(route);
                let yaml = std::fs::read_to_string(&path).unwrap();
                let resource: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
                let namespace = resource["metadata"]["namespace"].as_str().unwrap();
                assert_eq!(namespace, denied_backend_ref_namespace_for(k8s), "{}", path.display());
                let backend = &resource["spec"]["rules"][0]["backendRefs"][0]["namespace"];
                assert_eq!(backend.as_str(), Some(backend_namespace), "{}", path.display());
                assert_ne!(
                    backend.as_str(),
                    Some(namespace),
                    "fixture must exercise a cross-namespace ref"
                );
            }
        }
    }
}

/// Probe the Controller Admin admission boundary, not native Kubernetes conformance.
/// Authentication, transport failures and unexpected success must never pass.
pub async fn expect_admin_admission_rejected(ctx: &TestContext, resource: &Value) -> Result<(), String> {
    let kind = resource["kind"].as_str().ok_or("fixture kind missing")?;
    let namespace = resource["metadata"]["namespace"]
        .as_str()
        .ok_or("fixture namespace missing")?;
    let name = resource["metadata"]["name"].as_str().ok_or("fixture name missing")?;
    let collection = format!("{}/api/v1/namespaced/{kind}/{namespace}", ctx.admin_api_url());
    let object = format!("{collection}/{name}");
    let authenticated = |request: reqwest::RequestBuilder| match &ctx.admin_token {
        Some(token) => request.bearer_auth(token),
        None => request,
    };
    let before = authenticated(ctx.http_client.get(&object))
        .send()
        .await
        .map_err(|e| format!("Admission precondition GET failed: {e}"))?;
    if before.status() != reqwest::StatusCode::NOT_FOUND {
        return Err(format!(
            "Admission fixture must initially be absent; GET returned {}",
            before.status()
        ));
    }
    let response = authenticated(ctx.http_client.post(&collection).json(resource))
        .send()
        .await
        .map_err(|e| format!("Admission POST failed: {e}"))?;
    if response.status() != reqwest::StatusCode::BAD_REQUEST {
        let status = response.status();
        return Err(format!(
            "Expected Admin 400, got {status}: {}",
            response.text().await.unwrap_or_default()
        ));
    }
    let after = authenticated(ctx.http_client.get(&object))
        .send()
        .await
        .map_err(|e| format!("Admission nonpersistence GET failed: {e}"))?;
    if after.status() != reqwest::StatusCode::NOT_FOUND {
        return Err(format!(
            "Rejected resource must remain absent; GET returned {}",
            after.status()
        ));
    }
    Ok(())
}
