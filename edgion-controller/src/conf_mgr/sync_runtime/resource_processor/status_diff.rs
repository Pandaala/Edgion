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
//! 1. **Condition level** – `conditions_semantically_equal` compares `Vec<Condition>`
//!    ignoring `last_transition_time`, matching by `type_`.
//! 2. **Composite helpers** – `parents_semantically_equal`, `ancestors_semantically_equal`,
//!    `listeners_semantically_equal` for structured sub-statuses.
//! 3. **Top-level** – `status_semantically_changed` is the single entry point
//!    called by `processor.rs`. It deserializes old/new JSON into
//!    `serde_json::Value`, then delegates to type-specific comparators.

use edgion_resources::resources::common::{ParentReference, ParentStatusIdentity};
use serde_json::Value;

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

    match kind {
        "HTTPRoute" | "GRPCRoute" | "TCPRoute" | "TLSRoute" | "UDPRoute" | "EdgionTls" => {
            !parents_status_equal(&old, &new, resource_namespace)
        }
        "Gateway" => !gateway_status_equal(&old, &new),
        "BackendTLSPolicy" | "EdgionBackendTrafficPolicy" => !ancestors_status_equal(&old, &new),
        "EdgionAcme" | "LinkSys" => !conditions_and_fields_status_equal(&old, &new),
        // Simple condition-only resources:
        // GatewayClass, EdgionGatewayConfig, EdgionPlugins, EdgionStreamPlugins, EdgionConfigData
        _ => !conditions_only_status_equal(&old, &new),
    }
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
// Simple condition-only status (GatewayClass, EdgionGatewayConfig,
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
// Per-parent status (HTTPRoute, GRPCRoute, TCPRoute, TLSRoute, UDPRoute, EdgionTls)
// ============================================================================

fn parents_status_equal(old: &Value, new: &Value, resource_namespace: Option<&str>) -> bool {
    if [old, new].iter().any(|status| {
        !status.is_object()
            || status
                .get("parents")
                .is_some_and(|parents| !parents.is_null() && !parents.is_array())
    }) {
        return false;
    }
    let old_parents = old.get("parents").and_then(Value::as_array);
    let new_parents = new.get("parents").and_then(Value::as_array);

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
                    .map(|entry| serde_json::from_value(entry.get("parentRef")?.clone()).ok())
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
                            && parent_entry_equal(old_parent, new_parent)
                    });
                let Some(index) = matched else { return false };
                used[index] = true;
            }
            true
        }
    }
}

/// Compare a single parent status entry: controllerName + conditions.
fn parent_entry_equal(old: &Value, new: &Value) -> bool {
    if old.get("controllerName") != new.get("controllerName") {
        return false;
    }
    let old_conds = old.get("conditions").unwrap_or(&Value::Null);
    let new_conds = new.get("conditions").unwrap_or(&Value::Null);
    conditions_equal(old_conds, new_conds)
}

// ============================================================================
// Per-ancestor status (BackendTLSPolicy, EdgionBackendTrafficPolicy)
// ============================================================================

fn ancestors_status_equal(old: &Value, new: &Value) -> bool {
    let old_ancestors = old.get("ancestors").and_then(Value::as_array);
    let new_ancestors = new.get("ancestors").and_then(Value::as_array);

    match (old_ancestors, new_ancestors) {
        (None, None) => true,
        (Some(a), None) | (None, Some(a)) => a.is_empty(),
        (Some(old_arr), Some(new_arr)) => {
            if old_arr.len() != new_arr.len() {
                return false;
            }
            for new_anc in new_arr {
                let matched = old_arr.iter().find(|old_a| ancestor_ref_matches(old_a, new_anc));
                match matched {
                    None => return false,
                    Some(old_a) => {
                        if !ancestor_entry_equal(old_a, new_anc) {
                            return false;
                        }
                    }
                }
            }
            true
        }
    }
}

fn ancestor_ref_matches(a: &Value, b: &Value) -> bool {
    let a_ref = a.get("ancestorRef");
    let b_ref = b.get("ancestorRef");
    match (a_ref, b_ref) {
        (Some(ar), Some(br)) => ar.get("name") == br.get("name") && ar.get("namespace") == br.get("namespace"),
        (None, None) => true,
        _ => false,
    }
}

fn ancestor_entry_equal(old: &Value, new: &Value) -> bool {
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
mod tests {
    use super::*;

    #[test]
    fn malformed_parent_status_containers_are_not_equal() {
        for malformed in ["null", "[]", "1", r#"{"parents":{}}"#, r#"{"parents":"invalid"}"#] {
            for other in [malformed, "{}", r#"{"parents":[]}"#] {
                assert!(status_semantically_changed("HTTPRoute", malformed, other, Some("apps")));
                assert!(status_semantically_changed("HTTPRoute", other, malformed, Some("apps")));
            }
        }
    }

    #[test]
    fn parent_status_comparison_uses_canonical_owned_identity_for_every_family() {
        use serde_json::json;
        let entry = json!({"parentRef": {"name": "gw"}, "controllerName": "custom.example/controller",
            "conditions": [{"type": "Accepted", "status": "False", "reason": "Test", "message": "m",
                "observedGeneration": 1, "lastTransitionTime": "2026-01-01T00:00:00Z"}]});
        let old = json!({"parents": [entry.clone()]}).to_string();
        for kind in [
            "HTTPRoute",
            "GRPCRoute",
            "TCPRoute",
            "TLSRoute",
            "UDPRoute",
            "EdgionTls",
        ] {
            let changed = |entries: Vec<Value>| {
                status_semantically_changed(kind, &old, &json!({"parents": entries}).to_string(), Some("apps"))
            };
            let mut equivalent = entry.clone();
            equivalent["parentRef"] = json!({"name": "gw", "namespace": "apps",
                "kind": "Gateway", "group": "gateway.networking.k8s.io"});
            equivalent["conditions"][0]["lastTransitionTime"] = json!("2026-02-01T00:00:00Z");
            assert!(!changed(vec![equivalent]), "{kind}");
            for (field, value) in [
                ("group", json!("")),
                ("kind", json!("HTTPRoute")),
                ("namespace", json!("elsewhere")),
                ("name", json!("other")),
                ("sectionName", json!("https")),
                ("port", json!(443)),
            ] {
                let mut different = entry.clone();
                different["parentRef"][field] = value;
                assert!(changed(vec![different]), "{kind}: {field}");
            }
            let mut other_owner = entry.clone();
            other_owner["controllerName"] = json!("foreign.example/controller");
            assert!(changed(vec![other_owner]));
            let mut generation = entry.clone();
            generation["conditions"][0]["observedGeneration"] = json!(2);
            assert!(changed(vec![generation]));
            assert!(changed(vec![entry.clone(), entry.clone()]));
        }
    }

    #[test]
    fn parent_status_comparison_consumes_duplicate_entries_once_and_ignores_order() {
        use serde_json::json;
        let accepted = json!({"parentRef": {"name": "gw"}, "controllerName": "custom/controller",
            "conditions": [{"type": "Accepted", "status": "True"}]});
        let mut rejected = accepted.clone();
        rejected["conditions"][0]["status"] = json!("False");
        let old = json!({"parents": [accepted.clone(), rejected.clone()]}).to_string();
        let reordered = json!({"parents": [rejected, accepted.clone()]}).to_string();
        assert!(!status_semantically_changed(
            "HTTPRoute",
            &old,
            &reordered,
            Some("apps")
        ));
        let duplicated = json!({"parents": [accepted.clone(), accepted]}).to_string();
        assert!(status_semantically_changed(
            "HTTPRoute",
            &old,
            &duplicated,
            Some("apps")
        ));
        let malformed = json!({"parents": [{"parentRef": {"name": 1}, "controllerName": "c"}]}).to_string();
        assert!(status_semantically_changed(
            "HTTPRoute",
            &malformed,
            &malformed,
            Some("apps")
        ));
    }

    fn acme_status(ltt: &str, cond_status: &str, phase: &str) -> String {
        format!(
            r#"{{"conditions":[{{"type":"Accepted","status":"{cond_status}","reason":"Accepted","message":"ok","lastTransitionTime":"{ltt}","observedGeneration":1}}],"phase":"{phase}"}}"#
        )
    }

    #[test]
    fn acme_ltt_only_change_is_not_a_status_change() {
        let old = acme_status("2026-01-01T00:00:00Z", "True", "Ready");
        let new = acme_status("2026-07-19T00:00:00Z", "True", "Ready");
        assert!(
            !status_semantically_changed("EdgionAcme", &old, &new, None),
            "a lastTransitionTime-only difference must not trigger a status write"
        );
    }

    #[test]
    fn acme_condition_flip_is_a_status_change() {
        let old = acme_status("2026-01-01T00:00:00Z", "True", "Ready");
        let new = acme_status("2026-01-01T00:00:00Z", "False", "Ready");
        assert!(status_semantically_changed("EdgionAcme", &old, &new, None));
    }

    #[test]
    fn acme_non_condition_field_change_is_a_status_change() {
        let old = acme_status("2026-01-01T00:00:00Z", "True", "Ready");
        let new = acme_status("2026-01-01T00:00:00Z", "True", "Renewing");
        assert!(status_semantically_changed("EdgionAcme", &old, &new, None));
    }

    #[test]
    fn link_sys_ltt_only_change_is_not_a_status_change() {
        let old = r#"{"conditions":[{"type":"Accepted","status":"True","reason":"Accepted","message":"ok","lastTransitionTime":"2026-01-01T00:00:00Z","observedGeneration":1}],"credentialState":"Valid","publishedGeneration":"generation-1"}"#;
        let new = old.replace("2026-01-01", "2026-02-02");
        assert!(
            !status_semantically_changed("LinkSys", old, &new, None),
            "a lastTransitionTime-only difference must not trigger a status write"
        );
    }

    #[test]
    fn link_sys_credential_field_change_is_a_status_change() {
        let old = r#"{"conditions":[{"type":"Accepted","status":"True","reason":"Accepted","message":"ok","lastTransitionTime":"2026-01-01T00:00:00Z","observedGeneration":1}]}"#;
        let new = r#"{"conditions":[{"type":"Accepted","status":"True","reason":"Accepted","message":"ok","lastTransitionTime":"2026-01-01T00:00:00Z","observedGeneration":1}],"credentialState":"Valid","publishedGeneration":"generation-1","desiredConsumerCount":1,"publishedConsumerCount":1,"conflictedConsumerCount":0}"#;
        assert!(
            status_semantically_changed("LinkSys", old, new, None),
            "credential lifecycle fields must trigger a status write"
        );
    }

    #[test]
    fn link_sys_credential_field_clear_is_a_status_change() {
        let old = r#"{"conditions":[{"type":"Accepted","status":"True","reason":"Accepted","message":"ok","lastTransitionTime":"2026-01-01T00:00:00Z","observedGeneration":1}],"credentialState":"Valid","publishedGeneration":"generation-1"}"#;
        let new = r#"{"conditions":[{"type":"Accepted","status":"True","reason":"Accepted","message":"ok","lastTransitionTime":"2026-01-01T00:00:00Z","observedGeneration":1}]}"#;
        assert!(
            status_semantically_changed("LinkSys", old, new, None),
            "clearing stale credential lifecycle fields must trigger a status write"
        );
    }

    /// `EdgionBackendTrafficPolicy` (GEP-2648 `status.ancestors[]`, like
    /// `BackendTLSPolicy`) must dispatch through `ancestors_status_equal`: a
    /// `lastTransitionTime`-only change is not a semantic change, but a
    /// `status` flip on a condition is.
    #[test]
    fn ebtp_uses_ancestors_comparison() {
        let old = r#"{"ancestors":[{"ancestorRef":{"name":"gw"},"controllerName":"c","conditions":[{"type":"Accepted","status":"True","reason":"Accepted","message":"m","lastTransitionTime":"2026-01-01T00:00:00Z"}]}]}"#;
        let new_time_only = old.replace("2026-01-01", "2026-02-02");
        assert!(
            !status_semantically_changed("EdgionBackendTrafficPolicy", old, &new_time_only, None),
            "lastTransitionTime-only change must not trigger a status write"
        );
        let new_flipped = old.replace("\"status\":\"True\"", "\"status\":\"False\"");
        assert!(status_semantically_changed(
            "EdgionBackendTrafficPolicy",
            old,
            &new_flipped,
            None
        ));
    }
}
