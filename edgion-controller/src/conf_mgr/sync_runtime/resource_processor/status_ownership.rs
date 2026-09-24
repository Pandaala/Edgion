//! Kubernetes status writer ownership, separate from local serving diagnostics.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use edgion_resources::resources::common::ParentReference;
use edgion_resources::resources::edgion_acme::AcmeChallenge;
use edgion_resources::{EdgionAcme, GRPCRoute, Gateway, GatewayClass, HTTPRoute};
use serde_json::{json, Value};

use crate::conf_mgr::ControllerRuntime;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KubernetesStatusIdentity {
    pub controller_name: String,
}

/// Ownership requires the referenced class incarnation, not its conditions.
pub fn gateway_has_class_owner(gateway: &Gateway, controller_name: &str, runtime: &ControllerRuntime) -> bool {
    gateway_owned_class_uid(gateway, controller_name, runtime).is_some()
}

fn gateway_owned_class_uid(gateway: &Gateway, controller_name: &str, runtime: &ControllerRuntime) -> Option<String> {
    let name = &gateway.spec.gateway_class_name;
    let processor = runtime.processor_registry.get("GatewayClass")?;
    let raw = processor
        .get_raw_any(name)
        .and_then(|value| value.downcast::<GatewayClass>().ok())?;
    let published = processor
        .get_cached_any(name)
        .and_then(|value| value.downcast::<GatewayClass>().ok())?;
    let uid = raw.metadata.uid.as_deref().filter(|uid| !uid.is_empty())?;
    (raw.spec.controller_name == controller_name
        && published.spec.controller_name == controller_name
        && published.metadata.uid.as_deref() == Some(uid))
    .then(|| uid.to_owned())
}

pub fn gateway_is_owned(gateway: &Gateway, runtime: &ControllerRuntime) -> bool {
    runtime
        .status_identity()
        .is_none_or(|identity| gateway_has_class_owner(gateway, &identity.controller_name, runtime))
}

/// A serving attachment requires this deployment's current published incarnation.
/// Raw objects remain available separately for diagnostics, never serving authority.
pub fn serving_gateway(namespace: &str, name: &str, runtime: &ControllerRuntime) -> Option<Arc<Gateway>> {
    serving_gateway_with_class_uid(namespace, name, runtime).map(|(gateway, _)| gateway)
}

fn serving_gateway_with_class_uid(
    namespace: &str,
    name: &str,
    runtime: &ControllerRuntime,
) -> Option<(Arc<Gateway>, Option<String>)> {
    let processor = runtime.processor_registry.get("Gateway")?;
    let key = format!("{namespace}/{name}");
    let published = processor.get_cached_any(&key)?.downcast::<Gateway>().ok()?;
    let Some(identity) = runtime.status_identity() else {
        return Some((published, None));
    };
    let raw = processor.get_raw_any(&key)?.downcast::<Gateway>().ok()?;
    let uid = raw.metadata.uid.as_deref().filter(|uid| !uid.is_empty())?;
    let class_uid = gateway_owned_class_uid(&raw, &identity.controller_name, runtime)?;
    (published.metadata.uid.as_deref() == Some(uid) && published.spec.gateway_class_name == raw.spec.gateway_class_name)
        .then_some((published, Some(class_uid)))
}

pub fn gateway_publication_identity(gateway: &Gateway, runtime: &ControllerRuntime) -> Option<(String, String)> {
    let namespace = gateway.metadata.namespace.as_deref().unwrap_or("default");
    let name = gateway.metadata.name.as_deref()?;
    let (published, class_uid) = serving_gateway_with_class_uid(namespace, name, runtime)?;
    let uid = published.metadata.uid.as_deref().filter(|uid| !uid.is_empty())?;
    if gateway.metadata.uid.as_deref() != Some(uid) {
        return None;
    }
    // Use the exact class incarnation proved with the returned Gateway. A new
    // raw lookup here could stamp a not-yet-published class and suppress recovery.
    Some((uid.into(), class_uid?))
}

fn raw_gateway(parent: &ParentReference, namespace: &str, runtime: &ControllerRuntime) -> Option<Arc<Gateway>> {
    let identity = parent.canonical_identity(Some(namespace));
    if !identity.is_gateway() {
        return None;
    }
    runtime
        .processor_registry
        .get("Gateway")?
        .get_raw_any(&format!("{}/{}", identity.namespace, identity.name))?
        .downcast::<Gateway>()
        .ok()
}

pub fn parent_is_owned(parent: &ParentReference, namespace: &str, runtime: &ControllerRuntime) -> bool {
    runtime.status_identity().is_none()
        || raw_gateway(parent, namespace, runtime).is_some_and(|gateway| gateway_is_owned(&gateway, runtime))
}

pub fn acme_is_owned(acme: &EdgionAcme, runtime: &ControllerRuntime) -> bool {
    let AcmeChallenge::Http01 { gateway_ref } = &acme.spec.challenge;
    parent_is_owned(
        gateway_ref,
        acme.metadata.namespace.as_deref().unwrap_or("default"),
        runtime,
    )
}

type ParentKey = (String, String, String, String, Option<String>, Option<i32>);

fn parent_key(parent: &ParentReference, namespace: &str) -> ParentKey {
    let identity = parent.canonical_identity(Some(namespace));
    (
        identity.group.into(),
        identity.kind.into(),
        identity.namespace.into(),
        identity.name.into(),
        identity.section_name.map(str::to_owned),
        identity.port,
    )
}

fn is_reverse_lock(parent: &ParentReference, kind: &str, namespace: &str) -> bool {
    let identity = parent.canonical_identity(Some(namespace));
    matches!(kind, "HTTPRoute" | "GRPCRoute") && identity.group == "gateway.networking.k8s.io" && identity.kind == kind
}

/// Pick from the complete explicit spec BEFORE looking up the selected dependency.
/// Missing or unsupported first choices never fall back to another visible branch.
fn attachment_is_owned(parent: &ParentReference, kind: &str, namespace: &str, runtime: &ControllerRuntime) -> bool {
    let mut selected = parent.clone();
    let mut namespace = namespace.to_owned();
    let mut visited = HashSet::new();
    for depth in 0..=32 {
        let identity = selected.canonical_identity(Some(&namespace));
        if identity.is_gateway() {
            return parent_is_owned(&selected, &namespace, runtime);
        }
        if depth == 32 || !is_reverse_lock(&selected, kind, &namespace) {
            return false;
        }
        let key = format!("{}/{}", identity.namespace, identity.name);
        if !visited.insert(key.clone()) {
            return false;
        }
        let Some(processor) = runtime.processor_registry.get(kind) else {
            return false;
        };
        let Some(raw) = processor.get_raw_any(&key) else {
            return false;
        };
        let refs = match kind {
            "HTTPRoute" => raw
                .downcast::<HTTPRoute>()
                .ok()
                .and_then(|route| route.spec.parent_refs.clone()),
            "GRPCRoute" => raw
                .downcast::<GRPCRoute>()
                .ok()
                .and_then(|route| route.spec.parent_refs.clone()),
            _ => None,
        };
        namespace = identity.namespace.to_owned();
        let Some(first) = refs.and_then(|refs| refs.into_iter().min_by_key(|parent| parent_key(parent, &namespace)))
        else {
            return false;
        };
        selected = first;
    }
    false
}

/// Return distinct local and persisted native status snapshots.
/// Unknown ownership preserves historical API diagnostics, never serving authority.
pub fn native_status_projection(
    kind: &str,
    namespace: &str,
    raw_resource: &Value,
    observed_status: &Value,
    computed_status: &Value,
    runtime: &ControllerRuntime,
) -> Option<(Value, Value)> {
    let owner = runtime.status_identity()?;
    let (list, reference) = match kind {
        "HTTPRoute" | "GRPCRoute" | "TCPRoute" | "TLSRoute" | "UDPRoute" | "EdgionTls" => ("parents", "parentRef"),
        "BackendTLSPolicy" | "EdgionBackendTrafficPolicy" => ("ancestors", "ancestorRef"),
        _ => return None,
    };
    let raw_refs: Vec<ParentReference> = raw_resource
        .pointer("/spec/parentRefs")
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default();
    let own_entry =
        |entry: &Value| entry.get("controllerName").and_then(Value::as_str) == Some(owner.controller_name.as_str());
    let parse_ref = |entry: &Value| {
        entry
            .get(reference)
            .and_then(|value| serde_json::from_value::<ParentReference>(value.clone()).ok())
    };
    let mut local_entries = Vec::new();
    let mut own_entries = Vec::new();
    let mut seen = HashSet::new();
    let mut ownership = HashMap::new();
    for entry in computed_status
        .get(list)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if !own_entry(entry) {
            continue;
        }
        let Some(parent) = parse_ref(entry) else {
            continue;
        };
        let owned = *ownership
            .entry(parent_key(&parent, namespace))
            .or_insert_with(|| attachment_is_owned(&parent, kind, namespace, runtime));
        if owned || is_reverse_lock(&parent, kind, namespace) {
            local_entries.push(entry.clone());
        }
        if owned && seen.insert(parent_key(&parent, namespace)) {
            own_entries.push(entry.clone());
        }
    }
    let mut persisted_entries = Vec::new();
    for entry in observed_status
        .get(list)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if own_entry(entry) {
            if let Some(parent) = parse_ref(entry) {
                let removed_from_spec = list == "parents"
                    && !raw_refs
                        .iter()
                        .any(|current| parent_key(current, namespace) == parent_key(&parent, namespace));
                let owned = *ownership
                    .entry(parent_key(&parent, namespace))
                    .or_insert_with(|| attachment_is_owned(&parent, kind, namespace, runtime));
                if removed_from_spec || owned {
                    continue;
                }
            }
        }
        persisted_entries.push(entry.clone());
    }
    persisted_entries.extend(own_entries);
    let mut local = computed_status.as_object().cloned().unwrap_or_default();
    local.insert(list.into(), Value::Array(local_entries));
    let mut persisted = observed_status.as_object().cloned().unwrap_or_default();
    persisted.insert(list.into(), Value::Array(persisted_entries));
    Some((Value::Object(local), Value::Object(persisted)))
}

/// Source-commit dependency fingerprints exclude status/resourceVersion churn.
pub fn ownership_dependency_signature(kind: &str, resource: &Value) -> Option<Value> {
    let uid = resource.pointer("/metadata/uid");
    match kind {
        "GatewayClass" => Some(json!([uid, resource.pointer("/spec/controllerName")])),
        "Gateway" => Some(json!([uid, resource.pointer("/spec/gatewayClassName")])),
        "HTTPRoute" | "GRPCRoute" => {
            let namespace = resource
                .pointer("/metadata/namespace")
                .and_then(Value::as_str)
                .unwrap_or("default");
            let refs: Vec<ParentReference> = resource
                .pointer("/spec/parentRefs")
                .and_then(|value| serde_json::from_value(value.clone()).ok())
                .unwrap_or_default();
            let mut keys: Vec<_> = refs.iter().map(|parent| parent_key(parent, namespace)).collect();
            keys.sort_unstable();
            keys.dedup();
            Some(json!([uid, keys]))
        }
        _ => None,
    }
}

pub fn ownership_dependency_kinds(kind: &str) -> &'static [&'static str] {
    match kind {
        "GatewayClass" => &[
            "Gateway",
            "HTTPRoute",
            "GRPCRoute",
            "TCPRoute",
            "TLSRoute",
            "UDPRoute",
            "EdgionTls",
            "BackendTLSPolicy",
            "EdgionBackendTrafficPolicy",
            "EdgionAcme",
            "EdgionPlugins",
            "EdgionStreamPlugins",
            "EdgionBackend",
            "LinkSys",
        ],
        "Gateway" => &[
            "HTTPRoute",
            "GRPCRoute",
            "TCPRoute",
            "TLSRoute",
            "UDPRoute",
            "EdgionTls",
            "BackendTLSPolicy",
            "EdgionBackendTrafficPolicy",
            "EdgionAcme",
        ],
        "HTTPRoute" => &["HTTPRoute"],
        "GRPCRoute" => &["GRPCRoute"],
        _ => &[],
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/status_ownership_test.rs"]
mod tests;
