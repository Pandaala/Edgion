//! The final policy ancestor budget includes protected observed history.
use edgion_resources::resources::common::policy_ancestor_admission::{
    PreparedPolicyAncestorAdmission, MAX_POLICY_ANCESTORS,
};
use edgion_resources::resources::common::{ParentReference, PolicyAncestorStatus};
use edgion_resources::{BackendTLSPolicy, EdgionBackendTrafficPolicy};
use serde_json::{json, Value};
use std::any::Any;
use std::collections::{BTreeMap, HashSet};

type Key = (String, String, String, String, Option<String>, Option<i32>);
fn key(parent: &ParentReference, namespace: &str) -> Key {
    let p = parent.canonical_identity(Some(namespace));
    (
        p.group.into(),
        p.kind.into(),
        p.namespace.into(),
        p.name.into(),
        p.section_name.map(str::to_owned),
        p.port,
    )
}
fn reference(entry: &Value) -> Option<ParentReference> {
    serde_json::from_value(entry.get("ancestorRef")?.clone()).ok()
}

pub(super) struct Composition {
    pub local: Value,
    pub persisted: Value,
    pub persistable: bool,
}

pub(super) fn compose(
    namespace: &str,
    controller: &str,
    observed: &Value,
    computed: &Value,
    mut owned: impl FnMut(&ParentReference) -> bool,
) -> Composition {
    let own = |entry: &Value| entry.get("controllerName").and_then(Value::as_str) == Some(controller);
    let mut candidates = BTreeMap::new();
    for entry in computed
        .get("ancestors")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if own(entry) {
            if let Some(parent) = reference(entry).filter(|parent| owned(parent)) {
                candidates
                    .entry(key(&parent, namespace))
                    .or_insert_with(|| entry.clone());
            }
        }
    }
    let mut protected = Vec::new();
    let mut represented = HashSet::new();
    for entry in observed
        .get("ancestors")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if own(entry) {
            if let Some(parent) = reference(entry).filter(|parent| owned(parent)) {
                represented.insert(key(&parent, namespace));
                continue;
            }
        }
        protected.push(entry.clone());
    }
    let persistable = protected.len() <= MAX_POLICY_ANCESTORS;
    let slots = MAX_POLICY_ANCESTORS.saturating_sub(protected.len());
    let mut ordered: Vec<_> = candidates.into_iter().collect();
    ordered.sort_by(|(a, _), (b, _)| (!represented.contains(a), a).cmp(&(!represented.contains(b), b)));
    let local: Vec<_> = ordered.into_iter().take(slots).map(|(_, entry)| entry).collect();
    protected.extend(local.iter().cloned());
    let mut persisted = observed.as_object().cloned().unwrap_or_default();
    persisted.insert("ancestors".into(), Value::Array(protected));
    Composition {
        local: json!({"ancestors": local}),
        persisted: Value::Object(persisted),
        persistable,
    }
}

/// Stamp from the exact local projection, never from a second ownership lookup.
pub(super) fn stamp(resource: &mut dyn Any, namespace: &str, local: &Value, kubernetes: bool) {
    let admission = if kubernetes {
        let keys = local
            .get("ancestors")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(reference)
            .filter_map(|parent| {
                let identity = parent.canonical_identity(Some(namespace));
                identity
                    .is_gateway()
                    .then(|| format!("{}/{}", identity.namespace, identity.name))
            })
            .collect();
        PreparedPolicyAncestorAdmission::Scoped(keys).validated()
    } else {
        PreparedPolicyAncestorAdmission::Unrestricted
    };
    if let Some(policy) = resource.downcast_mut::<BackendTLSPolicy>() {
        policy.spec.prepared_ancestor_admission = Some(admission);
    } else if let Some(policy) = resource.downcast_mut::<EdgionBackendTrafficPolicy>() {
        policy.spec.prepared_ancestor_admission = Some(admission);
    }
}

/// Native handlers retain their configured owner and have no Kubernetes proof dependency.
pub(super) fn compose_native(
    namespace: &str,
    owner: &str,
    observed: &[PolicyAncestorStatus],
    computed: &mut Vec<PolicyAncestorStatus>,
) {
    let observed = json!({"ancestors": observed});
    let composition = compose(namespace, owner, &observed, &json!({"ancestors": computed}), |_| true);
    if let Ok(entries) = serde_json::from_value(composition.persisted["ancestors"].clone()) {
        *computed = entries;
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/policy_ancestor_capacity_test.rs"]
mod tests;
