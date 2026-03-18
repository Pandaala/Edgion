//! Status Diff — Semantic status comparison to minimize K8s API writes
//!
//! Every status write triggers a K8s API call (or a FileSystem write).
//! The previous approach serialized status to JSON strings and compared them
//! verbatim, which produced false positives when non-semantic fields like
//! `last_transition_time` changed even though the logical state was identical.
//!
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

use serde_json::Value;

/// Entry point: check whether the new status is semantically different from
/// the old status, given the resource `kind`.
///
/// Returns `true` if a write should be issued (i.e., status actually changed).
pub fn status_semantically_changed(kind: &str, old_json: &str, new_json: &str) -> bool {
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
            !parents_status_equal(&old, &new)
        }
        "Gateway" => !gateway_status_equal(&old, &new),
        "BackendTLSPolicy" => !ancestors_status_equal(&old, &new),
        "EdgionAcme" => old != new,
        // Simple condition-only resources:
        // GatewayClass, EdgionGatewayConfig, LinkSys, EdgionPlugins, EdgionStreamPlugins
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
    let old_arr = match old.as_array() {
        Some(a) => a,
        None => return new.as_array().map_or(true, |a| a.is_empty()),
    };
    let new_arr = match new.as_array() {
        Some(a) => a,
        None => return old_arr.is_empty(),
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
// Simple condition-only status (GatewayClass, EdgionGatewayConfig, LinkSys, etc.)
// ============================================================================

fn conditions_only_status_equal(old: &Value, new: &Value) -> bool {
    let old_conds = old.get("conditions").unwrap_or(&Value::Null);
    let new_conds = new.get("conditions").unwrap_or(&Value::Null);
    conditions_equal(old_conds, new_conds)
}

// ============================================================================
// Per-parent status (HTTPRoute, GRPCRoute, TCPRoute, TLSRoute, UDPRoute, EdgionTls)
// ============================================================================

fn parents_status_equal(old: &Value, new: &Value) -> bool {
    let old_parents = old.get("parents").and_then(Value::as_array);
    let new_parents = new.get("parents").and_then(Value::as_array);

    match (old_parents, new_parents) {
        (None, None) => true,
        (Some(a), None) | (None, Some(a)) => a.is_empty(),
        (Some(old_arr), Some(new_arr)) => {
            if old_arr.len() != new_arr.len() {
                return false;
            }
            for new_parent in new_arr {
                let matched = old_arr.iter().find(|old_p| parent_ref_matches(old_p, new_parent));
                match matched {
                    None => return false,
                    Some(old_p) => {
                        if !parent_entry_equal(old_p, new_parent) {
                            return false;
                        }
                    }
                }
            }
            true
        }
    }
}

/// Match parent entries by parentRef identity (name + namespace + sectionName).
fn parent_ref_matches(a: &Value, b: &Value) -> bool {
    let a_ref = a.get("parentRef");
    let b_ref = b.get("parentRef");
    match (a_ref, b_ref) {
        (Some(ar), Some(br)) => {
            ar.get("name") == br.get("name")
                && ar.get("namespace") == br.get("namespace")
                && ar.get("sectionName") == br.get("sectionName")
        }
        (None, None) => true,
        _ => false,
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
// Per-ancestor status (BackendTLSPolicy)
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
