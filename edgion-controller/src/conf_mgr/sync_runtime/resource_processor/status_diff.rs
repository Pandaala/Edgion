//! Status Diff — Semantic status comparison to minimize K8s API writes
//!
//! Every status write triggers a K8s API call (or a FileSystem write).
//! This module provides semantic comparison functions that ignore
//! `last_transition_time` (and similar timestamp fields) so that a status
//! write is only triggered when the user-visible meaning actually changes.
//!
//! ## Architecture
//!
//! Instead of a single trait, we use a layered approach:
//!
//! 1. **Condition level** – `conditions_equal` compares condition arrays,
//!    ignoring `lastTransitionTime`, matching by `type`.
//! 2. **Composite helpers** – `attachment_statuses_equal` compares parents and
//!    ancestors; `gateway_status_equal` includes listener status.
//! 3. **Top-level** – `status_semantically_changed` is the single entry point
//!    called by `processor.rs`. It deserializes old/new JSON into
//!    `serde_json::Value`, then delegates to type-specific comparators.

use edgion_resources::resources::common::ParentReference;
use edgion_resources::resources::refs::ParentStatusIdentity;
use serde_json::Value;
use std::collections::BTreeMap;

/// Entry point: check whether the new status is semantically different from
/// the old status, given the resource `kind`.
///
/// Returns `true` if a write should be issued (i.e., status actually changed).
pub fn status_semantically_changed(
    kind: &str,
    old_json: &str,
    new_json: &str,
    resource_namespace: Option<&str>,
) -> bool {
    let old: Value = match serde_json::from_str(old_json) {
        Ok(v) => v,
        Err(_) => return true,
    };
    let new: Value = match serde_json::from_str(new_json) {
        Ok(v) => v,
        Err(_) => return true,
    };

    if super::status_observation::is_partitioned_kind(kind)
        && (old.get("controllers").is_some() || new.get("controllers").is_some())
    {
        return !controller_observations_equal(kind, &old, &new);
    }

    !native_status_equal(kind, &old, &new, resource_namespace)
}

fn native_status_equal(kind: &str, old: &Value, new: &Value, resource_namespace: Option<&str>) -> bool {
    match kind {
        "HTTPRoute" | "GRPCRoute" | "TCPRoute" | "TLSRoute" | "UDPRoute" | "EdgionTls" => {
            attachment_statuses_equal(old, new, resource_namespace, "parents", "parentRef")
        }
        "Gateway" => gateway_status_equal(old, new),
        "BackendTLSPolicy" | "EdgionBackendTrafficPolicy" => {
            attachment_statuses_equal(old, new, resource_namespace, "ancestors", "ancestorRef")
        }
        "EdgionAcme" | "LinkSys" | "GatewayClass" => conditions_and_fields_status_equal(old, new),
        // Simple condition-only resources:
        // EdgionGatewayConfig, EdgionPlugins, EdgionStreamPlugins, EdgionConfigData
        _ => conditions_only_status_equal(old, new),
    }
}

/// Kubernetes observations are a map keyed by deployment identity, not array order.
fn controller_observations_equal(kind: &str, old: &Value, new: &Value) -> bool {
    type Observations<'a> = BTreeMap<&'a str, &'a Value>;
    fn decode(value: &Value) -> Option<Observations<'_>> {
        let object = value.as_object()?;
        if object.len() != 1 {
            return None;
        }
        let mut entries = BTreeMap::new();
        for entry in object.get("controllers")?.as_array()? {
            let controller = entry.get("controllerName")?.as_str()?;
            if entry
                .as_object()?
                .keys()
                .any(|field| !matches!(field.as_str(), "controllerName" | "status"))
            {
                return None;
            }
            let status = entry.get("status")?;
            if controller.is_empty() || !status.is_object() || entries.insert(controller, status).is_some() {
                return None;
            }
        }
        Some(entries)
    }
    let (Some(old), Some(new)) = (decode(old), decode(new)) else {
        return false;
    };
    old.len() == new.len()
        && new.iter().all(|(key, payload)| {
            old.get(key)
                .is_some_and(|old_payload| native_status_equal(kind, old_payload, payload, None))
        })
}

// ============================================================================
// Condition-level comparison
// ============================================================================

/// Compare two condition arrays semantically.
///
/// Two condition arrays are equal if:
/// - They contain the same set of condition types
/// - For each type, `status`, `reason`, `message`, and `observedGeneration` match
/// - `lastTransitionTime` is intentionally ignored
fn conditions_equal(old: &Value, new: &Value) -> bool {
    let Some(old_arr) = old.as_array() else {
        return new.as_array().is_none_or(|a| a.is_empty());
    };
    let Some(new_arr) = new.as_array() else {
        return old_arr.is_empty();
    };

    if old_arr.len() != new_arr.len() {
        return false;
    }

    for new_cond in new_arr {
        let cond_type = new_cond.get("type").and_then(Value::as_str).unwrap_or("");
        let old_cond = old_arr
            .iter()
            .find(|c| c.get("type").and_then(Value::as_str).unwrap_or("") == cond_type);

        match old_cond {
            None => return false,
            Some(old_c) => {
                if !condition_fields_equal(old_c, new_cond) {
                    return false;
                }
            }
        }
    }

    true
}

/// Compare semantic fields of two conditions (ignoring `lastTransitionTime`).
fn condition_fields_equal(a: &Value, b: &Value) -> bool {
    let fields = ["type", "status", "reason", "message", "observedGeneration"];
    for field in &fields {
        if a.get(field) != b.get(field) {
            return false;
        }
    }
    true
}

// ============================================================================
// Simple condition-only status (EdgionGatewayConfig,
// EdgionPlugins, EdgionStreamPlugins, EdgionConfigData, etc.)
// ============================================================================

fn conditions_only_status_equal(old: &Value, new: &Value) -> bool {
    let old_conds = old.get("conditions").unwrap_or(&Value::Null);
    let new_conds = new.get("conditions").unwrap_or(&Value::Null);
    conditions_equal(old_conds, new_conds)
}

// ============================================================================
// Hybrid status (semantic conditions + structurally compared lifecycle fields)
// ============================================================================

/// Conditions are compared semantically (ignoring `lastTransitionTime`);
/// every other status field is compared structurally. A raw whole-blob
/// comparison would include condition `lastTransitionTime` and turn every
/// reconcile into a "changed" status write under FileSystem/Etcd backends.
fn conditions_and_fields_status_equal(old: &Value, new: &Value) -> bool {
    let old_conds = old.get("conditions").unwrap_or(&Value::Null);
    let new_conds = new.get("conditions").unwrap_or(&Value::Null);
    if !conditions_equal(old_conds, new_conds) {
        return false;
    }
    let strip = |v: &Value| -> Value {
        let mut v = v.clone();
        if let Some(map) = v.as_object_mut() {
            map.remove("conditions");
        }
        v
    };
    strip(old) == strip(new)
}

// ============================================================================
// Shared per-parent / per-ancestor status comparison
// ============================================================================

fn attachment_statuses_equal(
    old: &Value,
    new: &Value,
    resource_namespace: Option<&str>,
    list_field: &str,
    reference_field: &str,
) -> bool {
    if [old, new].iter().any(|status| {
        !status.is_object()
            || status
                .get(list_field)
                .is_some_and(|parents| !parents.is_null() && !parents.is_array())
    }) {
        return false;
    }
    let old_parents = old.get(list_field).and_then(Value::as_array);
    let new_parents = new.get(list_field).and_then(Value::as_array);

    match (old_parents, new_parents) {
        (None, None) => true,
        (Some(a), None) | (None, Some(a)) => a.is_empty(),
        (Some(old_arr), Some(new_arr)) => {
            if old_arr.len() != new_arr.len() {
                return false;
            }
            let decode_refs = |entries: &[Value]| -> Option<Vec<ParentReference>> {
                entries
                    .iter()
                    .map(|entry| serde_json::from_value(entry.get(reference_field)?.clone()).ok())
                    .collect()
            };
            let (Some(old_refs), Some(new_refs)) = (decode_refs(old_arr), decode_refs(new_arr)) else {
                return false;
            };
            let mut used = vec![false; old_arr.len()];
            for (new_parent, new_ref) in new_arr.iter().zip(&new_refs) {
                let Some(controller_name) = new_parent.get("controllerName").and_then(Value::as_str) else {
                    return false;
                };
                let identity = ParentStatusIdentity {
                    parent: new_ref.canonical_identity(resource_namespace),
                    controller_name,
                };
                // Match a multiset, not a first-match lookup that can reuse one old entry.
                let matched = old_arr
                    .iter()
                    .zip(&old_refs)
                    .enumerate()
                    .position(|(i, (old_parent, old_ref))| {
                        !used[i]
                            && old_parent.get("controllerName").and_then(Value::as_str)
                                == Some(identity.controller_name)
                            && old_ref.canonical_identity(resource_namespace) == identity.parent
                            && attachment_entry_equal(old_parent, new_parent)
                    });
                let Some(index) = matched else { return false };
                used[index] = true;
            }
            true
        }
    }
}

/// Compare a single attachment status entry: controllerName + conditions.
fn attachment_entry_equal(old: &Value, new: &Value) -> bool {
    if old.get("controllerName") != new.get("controllerName") {
        return false;
    }
    let old_conds = old.get("conditions").unwrap_or(&Value::Null);
    let new_conds = new.get("conditions").unwrap_or(&Value::Null);
    conditions_equal(old_conds, new_conds)
}

// ============================================================================
// Gateway status (hybrid: addresses + conditions + listeners)
// ============================================================================

fn gateway_status_equal(old: &Value, new: &Value) -> bool {
    // 1. Compare addresses
    if old.get("addresses") != new.get("addresses") {
        return false;
    }

    // 2. Compare gateway-level conditions
    let old_conds = old.get("conditions").unwrap_or(&Value::Null);
    let new_conds = new.get("conditions").unwrap_or(&Value::Null);
    if !conditions_equal(old_conds, new_conds) {
        return false;
    }

    // 3. Compare listeners
    let old_listeners = old.get("listeners").and_then(Value::as_array);
    let new_listeners = new.get("listeners").and_then(Value::as_array);

    match (old_listeners, new_listeners) {
        (None, None) => true,
        (Some(a), None) | (None, Some(a)) => a.is_empty(),
        (Some(old_arr), Some(new_arr)) => {
            if old_arr.len() != new_arr.len() {
                return false;
            }
            for new_ls in new_arr {
                let ls_name = new_ls.get("name").and_then(Value::as_str).unwrap_or("");
                let old_ls = old_arr
                    .iter()
                    .find(|l| l.get("name").and_then(Value::as_str).unwrap_or("") == ls_name);
                match old_ls {
                    None => return false,
                    Some(old_l) => {
                        if !listener_equal(old_l, new_ls) {
                            return false;
                        }
                    }
                }
            }
            true
        }
    }
}

/// Compare a single listener status: supportedKinds + attachedRoutes + conditions.
fn listener_equal(old: &Value, new: &Value) -> bool {
    if old.get("supportedKinds") != new.get("supportedKinds") {
        return false;
    }
    if old.get("attachedRoutes") != new.get("attachedRoutes") {
        return false;
    }
    let old_conds = old.get("conditions").unwrap_or(&Value::Null);
    let new_conds = new.get("conditions").unwrap_or(&Value::Null);
    conditions_equal(old_conds, new_conds)
}

#[cfg(test)]
#[path = "../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/status_diff_test.rs"]
mod tests;
