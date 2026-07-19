//! Common configuration types shared across all configuration center backends
//!
//! This module contains types that are used by both FileSystem and Kubernetes modes.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::conf_mgr::conf_center::traits::ConfWriterError;

use edgion_resources::resources::common::Condition;

// ============================================================================
// Shared controller constants and helpers (FileSystem and Etcd backends)
// ============================================================================

/// Default controller name used by FileSystem and Etcd backends.
pub const DEFAULT_CONTROLLER_NAME: &str = "edgion.io/gateway-controller";

/// Returns the Phase-1 foundation resource kinds for FileSystem/Etcd backends.
///
/// These must be ready before Phase-2 resources start processing, because
/// Phase-2 resources look up GatewayClass, Gateway, Secret, Service, etc.
/// during their first `handle()` call.
///
/// The K8s backend has its own variant that additionally filters `no_watch_kinds`.
pub fn phase1_kinds() -> Vec<&'static str> {
    vec![
        "GatewayClass",
        "Gateway",
        "Secret",
        "ConfigMap",
        "ReferenceGrant",
        "Service",
        "EndpointSlice",
    ]
}

// ============================================================================
// Shared status types (used by both FileSystem and Etcd backends)
// ============================================================================

/// Condition type used exclusively by [`ErrorStatus`] (parse-failure fallback).
///
/// No resource handler manages a condition of this type; its presence in a
/// persisted status marks the status as an ErrorStatus sidecar (see
/// `seed_existing_status`, which must not seed it back into a recovered
/// resource).
pub const ERROR_STATUS_CONDITION_TYPE: &str = "Ready";

/// Simple error status structure (for parse errors, etc.)
///
/// Used when the resource can't be fully parsed and we can't extract native status.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorStatus {
    /// Standard K8s conditions
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<Condition>,
}

impl ErrorStatus {
    /// Create an error status
    pub fn error(reason: &str, message: &str) -> Self {
        Self {
            conditions: vec![Condition {
                type_: ERROR_STATUS_CONDITION_TYPE.to_string(),
                status: "False".to_string(),
                reason: reason.to_string(),
                message: message.to_string(),
                last_transition_time: Utc::now().to_rfc3339(),
                observed_generation: None,
            }],
        }
    }

    /// Build the error status to persist, compared against the previously
    /// persisted status JSON (if any).
    ///
    /// Returns `None` when the stored status already carries a semantically
    /// identical error condition (same type/status/reason/message) — the
    /// caller must skip the write so the stored `lastTransitionTime` stays
    /// stable across re-scans of the same broken resource. When the condition
    /// differs but its `status` did not flip, the stored `lastTransitionTime`
    /// is carried over (transition-only LTT contract).
    pub fn error_for_persist(reason: &str, message: &str, existing_json: Option<&str>) -> Option<Self> {
        let mut status = Self::error(reason, message);
        let Some(existing) = existing_json.and_then(|j| serde_json::from_str::<serde_json::Value>(j).ok()) else {
            return Some(status);
        };
        let new_cond = &mut status.conditions[0];
        let old_cond = existing.get("conditions").and_then(|c| c.as_array()).and_then(|arr| {
            arr.iter()
                .find(|c| c.get("type").and_then(|t| t.as_str()) == Some(&new_cond.type_))
        });
        let Some(old_cond) = old_cond else {
            return Some(status);
        };
        let field = |name: &str| old_cond.get(name).and_then(|v| v.as_str()).unwrap_or("");
        if field("status") == new_cond.status {
            // No transition: keep the stored lastTransitionTime.
            let old_ltt = field("lastTransitionTime");
            if !old_ltt.is_empty() {
                new_cond.last_transition_time = old_ltt.to_string();
            }
            if field("reason") == new_cond.reason && field("message") == new_cond.message {
                return None;
            }
        }
        Some(status)
    }
}

// ============================================================================
// Shared resource parsing (used by both FileSystem and Etcd backends)
// ============================================================================

use edgion_resources::prelude_resources::{BackendTLSPolicy, BackendTLSPolicyAlpha3, TLSRoute, TLSRouteAlpha3};
use edgion_resources::{ResourceKind, ResourceMeta};
use kube::Resource;
use serde::de::DeserializeOwned;
use std::fmt::Debug;

/// Parse YAML content into a resource, with legacy API version support.
///
/// Handles transparent conversion of `v1alpha3` resources (BackendTLSPolicy, TLSRoute)
/// to their canonical `v1` types.
pub fn parse_resource_content<K>(content: &str) -> serde_yaml::Result<K>
where
    K: ResourceMeta + Resource + Clone + Send + Sync + Debug + serde::Serialize + DeserializeOwned + 'static,
{
    let kind = K::resource_kind();

    if kind != ResourceKind::BackendTLSPolicy && kind != ResourceKind::TLSRoute {
        serde_yaml::from_str::<K>(content)
    } else {
        match extract_api_version(content).as_deref() {
            Some("gateway.networking.k8s.io/v1alpha3") if kind == ResourceKind::BackendTLSPolicy => {
                let legacy: BackendTLSPolicyAlpha3 = serde_yaml::from_str(content)?;
                let canonical: BackendTLSPolicy = legacy.into();
                let normalized = serde_yaml::to_string(&canonical)?;
                serde_yaml::from_str::<K>(&normalized)
            }
            Some("gateway.networking.k8s.io/v1alpha3") if kind == ResourceKind::TLSRoute => {
                let legacy: TLSRouteAlpha3 = serde_yaml::from_str(content)?;
                let canonical: TLSRoute = legacy.into();
                let normalized = serde_yaml::to_string(&canonical)?;
                serde_yaml::from_str::<K>(&normalized)
            }
            _ => serde_yaml::from_str::<K>(content),
        }
    }
}

fn extract_api_version(content: &str) -> Option<String> {
    let value: serde_yaml::Value = serde_yaml::from_str(content).ok()?;
    value.get("apiVersion")?.as_str().map(ToString::to_string)
}

/// Merge an operator update with the server-owned portions of the current resource.
///
/// The request is authoritative for the resource body, including `spec` and
/// top-level operator fields such as ConfigMap `data`. Only labels and
/// annotations are mutable inside metadata. Identity, all other current
/// metadata (including Kubernetes lifecycle fields), and status remain
/// server-owned. Starting from the request is important: controller-resolved
/// fields that happen to live below the current `spec` must never be copied
/// back into an operator update.
pub(crate) fn merge_operator_update_values(current: Value, requested: Value) -> Result<Value, ConfWriterError> {
    let current_obj = current
        .as_object()
        .ok_or_else(|| ConfWriterError::ValidationError("current resource must be an object".to_string()))?;
    let requested_obj = requested
        .as_object()
        .ok_or_else(|| ConfWriterError::ValidationError("updated resource must be an object".to_string()))?;

    let current_meta = current_obj
        .get("metadata")
        .and_then(Value::as_object)
        .ok_or_else(|| ConfWriterError::ValidationError("current resource metadata must be an object".to_string()))?;
    let requested_meta = requested_obj
        .get("metadata")
        .and_then(Value::as_object)
        .ok_or_else(|| ConfWriterError::ValidationError("updated resource metadata must be an object".to_string()))?;

    let mut merged = requested_obj.clone();

    // API identity is immutable. Copying it from the current resource also
    // prevents a request from changing a dynamic Kubernetes endpoint's GVK.
    for key in ["apiVersion", "kind"] {
        match current_obj.get(key) {
            Some(value) => {
                merged.insert(key.to_string(), value.clone());
            }
            None => {
                merged.remove(key);
            }
        }
    }

    // Preserve every server-created metadata field, while making the two
    // operator-owned maps exact replacements (absence clears the map).
    let mut merged_meta = current_meta.clone();
    for key in ["labels", "annotations"] {
        match requested_meta.get(key) {
            Some(value) => {
                merged_meta.insert(key.to_string(), value.clone());
            }
            None => {
                merged_meta.remove(key);
            }
        }
    }
    merged.insert("metadata".to_string(), Value::Object(merged_meta));

    match current_obj.get("status") {
        Some(status) => {
            merged.insert("status".to_string(), status.clone());
        }
        None => {
            merged.remove("status");
        }
    }

    Ok(Value::Object(merged))
}

/// YAML adapter for non-Kubernetes configuration centers.
///
/// The Admin API assigns a fresh `metadata.resourceVersion` before calling a
/// FileSystem or etcd writer. Unlike Kubernetes replace, where the current
/// resourceVersion is the optimistic-concurrency token, local backends must
/// persist that newly assigned request value so successful updates remain
/// monotonically versioned. Direct storage callers that omit it retain the
/// current value.
///
/// `metadata.generation` follows the same rule for the same reason: no local
/// backend advances it, so it is operator-supplied here rather than
/// server-owned as it is on the Kubernetes path. Dropping the requested value
/// would freeze generation at its create-time value forever, stalling every
/// `observedGeneration` reconciliation, and would diverge from the file-watcher
/// path, which takes generation straight from the operator's YAML.
pub(crate) fn merge_operator_update_yaml(current: &str, requested: &str) -> Result<String, ConfWriterError> {
    let current: Value = serde_yaml::from_str(current).map_err(|source| ConfWriterError::ParseError {
        context: "Failed to parse current resource for update",
        source: source.into(),
    })?;
    let requested: Value = serde_yaml::from_str(requested).map_err(|source| ConfWriterError::ParseError {
        context: "Failed to parse updated resource",
        source: source.into(),
    })?;
    let requested_meta = requested.get("metadata").and_then(Value::as_object);
    let operator_owned: Vec<(String, Value)> = ["resourceVersion", "generation"]
        .into_iter()
        .filter_map(|key| {
            requested_meta
                .and_then(|metadata| metadata.get(key))
                .map(|value| (key.to_string(), value.clone()))
        })
        .collect();
    let mut merged = merge_operator_update_values(current, requested)?;
    if !operator_owned.is_empty() {
        let metadata = merged
            .get_mut("metadata")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                ConfWriterError::ValidationError("merged resource metadata must be an object".to_string())
            })?;
        for (key, value) in operator_owned {
            metadata.insert(key, value);
        }
    }
    serde_yaml::to_string(&merged).map_err(|source| ConfWriterError::ParseError {
        context: "Failed to serialize merged resource update",
        source: source.into(),
    })
}

/// Enforce an optional client precondition against the resource read inside
/// the backend's atomic update window.
pub(crate) fn ensure_expected_resource_version(current: &str, expected: Option<&str>) -> Result<(), ConfWriterError> {
    let current: Value = serde_yaml::from_str(current).map_err(|source| ConfWriterError::ParseError {
        context: "Failed to parse current resourceVersion",
        source: source.into(),
    })?;
    ensure_expected_resource_version_value(&current, expected)
}

pub(crate) fn ensure_expected_resource_version_value(
    current: &Value,
    expected: Option<&str>,
) -> Result<(), ConfWriterError> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let actual = current
        .get("metadata")
        .and_then(Value::as_object)
        .and_then(|metadata| metadata.get("resourceVersion"))
        .and_then(Value::as_str);
    if actual != Some(expected) {
        return Err(ConfWriterError::Conflict(format!(
            "resourceVersion precondition failed: expected {expected}, current {}",
            actual.unwrap_or("<missing>")
        )));
    }
    Ok(())
}

#[cfg(test)]
mod error_status_tests {
    use super::*;

    fn stored(status: &str, reason: &str, message: &str, ltt: &str) -> String {
        format!(
            r#"{{"conditions":[{{"type":"Ready","status":"{status}","reason":"{reason}","message":"{message}","lastTransitionTime":"{ltt}"}}]}}"#
        )
    }

    #[test]
    fn identical_error_skips_write() {
        let existing = stored("False", "ParseError", "bad yaml", "2026-01-01T00:00:00Z");
        assert!(
            ErrorStatus::error_for_persist("ParseError", "bad yaml", Some(&existing)).is_none(),
            "re-scanning the same broken resource must not rewrite its status"
        );
    }

    #[test]
    fn changed_message_writes_but_preserves_ltt() {
        let existing = stored("False", "ParseError", "bad yaml at line 1", "2026-01-01T00:00:00Z");
        let status = ErrorStatus::error_for_persist("ParseError", "bad yaml at line 9", Some(&existing))
            .expect("a different message must be persisted");
        assert_eq!(
            status.conditions[0].last_transition_time, "2026-01-01T00:00:00Z",
            "status did not flip, so lastTransitionTime must be carried over"
        );
    }

    #[test]
    fn no_prior_status_writes_fresh() {
        let status = ErrorStatus::error_for_persist("ParseError", "bad yaml", None).expect("first error must persist");
        assert_eq!(status.conditions[0].status, "False");
        assert!(!status.conditions[0].last_transition_time.is_empty());
    }
}

#[cfg(test)]
mod update_merge_tests {
    use super::*;

    #[test]
    fn operator_update_preserves_server_fields_without_restoring_current_spec() {
        let current = serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "HTTPRoute",
            "metadata": {
                "name": "route",
                "namespace": "default",
                "uid": "uid-1",
                "resourceVersion": "42",
                "generation": 7,
                "managedFields": [{"manager": "controller"}],
                "creationTimestamp": "2026-07-15T00:00:00Z",
                "deletionTimestamp": "2026-07-16T00:00:00Z",
                "ownerReferences": [{"name": "owner"}],
                "finalizers": ["edgion.io/finalizer"],
                "labels": {"old": "label"},
                "annotations": {"old": "annotation"}
            },
            "spec": {
                "hostnames": ["old.example"],
                "resolvedListeners": [{"name": "runtime-only"}]
            },
            "status": {"conditions": [{"type": "Accepted", "status": "True"}]}
        });
        let requested = serde_json::json!({
            "apiVersion": "spoofed/v9",
            "kind": "Gateway",
            "metadata": {
                "name": "renamed",
                "namespace": "other",
                "uid": "spoofed",
                "resourceVersion": "999",
                "finalizers": [],
                "labels": {"new": "label"}
            },
            "spec": {"hostnames": ["new.example"]},
            "status": {"conditions": []},
            "data": {"operator": "field"}
        });

        let merged = merge_operator_update_values(current, requested).expect("merge must succeed");

        assert_eq!(merged["apiVersion"], "gateway.networking.k8s.io/v1");
        assert_eq!(merged["kind"], "HTTPRoute");
        assert_eq!(merged["metadata"]["name"], "route");
        assert_eq!(merged["metadata"]["namespace"], "default");
        assert_eq!(merged["metadata"]["uid"], "uid-1");
        assert_eq!(merged["metadata"]["resourceVersion"], "42");
        assert_eq!(merged["metadata"]["generation"], 7);
        assert_eq!(merged["metadata"]["managedFields"][0]["manager"], "controller");
        assert_eq!(merged["metadata"]["creationTimestamp"], "2026-07-15T00:00:00Z");
        assert_eq!(merged["metadata"]["deletionTimestamp"], "2026-07-16T00:00:00Z");
        assert_eq!(merged["metadata"]["ownerReferences"][0]["name"], "owner");
        assert_eq!(merged["metadata"]["finalizers"][0], "edgion.io/finalizer");
        assert_eq!(merged["metadata"]["labels"]["new"], "label");
        assert!(merged["metadata"].get("annotations").is_none());
        assert_eq!(merged["spec"]["hostnames"][0], "new.example");
        assert!(merged["spec"].get("resolvedListeners").is_none());
        assert_eq!(merged["status"]["conditions"][0]["type"], "Accepted");
        assert_eq!(merged["data"]["operator"], "field");
    }

    #[test]
    fn non_kubernetes_yaml_update_uses_generated_request_resource_version() {
        let current = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: route
  namespace: default
  resourceVersion: "42"
  uid: uid-1
spec:
  hostnames: [old.example]
"#;
        let requested = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: route
  namespace: default
  resourceVersion: "43"
spec:
  hostnames: [new.example]
"#;

        let merged = merge_operator_update_yaml(current, requested).expect("non-Kubernetes merge must succeed");
        let merged: Value = serde_yaml::from_str(&merged).expect("merged resource must remain valid YAML");
        assert_eq!(merged["metadata"]["resourceVersion"], "43");
        assert_eq!(merged["metadata"]["uid"], "uid-1");
        assert_eq!(merged["spec"]["hostnames"][0], "new.example");
    }

    #[test]
    fn non_kubernetes_yaml_update_uses_requested_generation() {
        let current = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: gw
  namespace: default
  generation: 1
  uid: uid-1
spec:
  listeners: [{name: http}]
"#;
        let requested = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: gw
  namespace: default
  generation: 11
spec:
  listeners: [{name: http-v10}]
"#;

        let merged = merge_operator_update_yaml(current, requested).expect("non-Kubernetes merge must succeed");
        let merged: Value = serde_yaml::from_str(&merged).expect("merged resource must remain valid YAML");
        assert_eq!(merged["metadata"]["generation"], 11);
        assert_eq!(merged["metadata"]["uid"], "uid-1");
        assert_eq!(merged["spec"]["listeners"][0]["name"], "http-v10");
    }

    #[test]
    fn non_kubernetes_yaml_update_retains_generation_when_request_omits_it() {
        let current = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: gw
  namespace: default
  generation: 7
spec:
  listeners: [{name: http}]
"#;
        let requested = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: gw
  namespace: default
spec:
  listeners: [{name: http-v1}]
"#;

        let merged = merge_operator_update_yaml(current, requested).expect("non-Kubernetes merge must succeed");
        let merged: Value = serde_yaml::from_str(&merged).expect("merged resource must remain valid YAML");
        assert_eq!(merged["metadata"]["generation"], 7);
    }
}
