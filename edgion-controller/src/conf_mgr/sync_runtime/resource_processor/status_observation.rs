//! Kubernetes status projection and merge for controller-partitioned resources.

use std::collections::HashSet;

use anyhow::{bail, Result};
use serde_json::{json, Value};

use edgion_resources::resources::common::ConditionsStatus;
use edgion_resources::resources::common::MAX_CONTROLLER_OBSERVATIONS;
use edgion_resources::resources::link_sys::LinkSysLocalStatus;

/// The controller and GatewayClass incarnation that owns a local observation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObserverIdentity {
    pub controller_name: String,
    pub gateway_class_name: String,
    pub gateway_class_uid: String,
}

/// Authoritative GatewayClass evidence used to prune obsolete observations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClassObservation {
    Existing {
        name: String,
        uid: String,
        controller_name: String,
    },
    Deleted {
        name: String,
    },
}

pub fn is_partitioned_kind(kind: &str) -> bool {
    matches!(
        kind,
        "EdgionPlugins" | "EdgionStreamPlugins" | "EdgionBackend" | "LinkSys"
    )
}

/// Return the exact local observation, never a legacy flat status fallback.
pub fn project_status(kind: &str, status: &Value, identity: &ObserverIdentity) -> Result<Option<Value>> {
    if !is_partitioned_kind(kind) || status.is_null() {
        return Ok(None);
    }
    if status.as_object().is_some_and(|object| object.is_empty()) {
        return Ok(None);
    }
    if !status
        .as_object()
        .is_some_and(|object| object.contains_key("controllers"))
    {
        retired_native_fields(kind, status)?;
        return Ok(None);
    }
    let entries = parse_envelope(status)?;
    let local = entries.into_iter().find_map(|entry| {
        (entry_controller_name(&entry) == identity.controller_name
            && entry_gateway_class_name(&entry) == identity.gateway_class_name
            && entry_gateway_class_uid(&entry) == identity.gateway_class_uid)
            .then(|| entry.get("status").cloned())
            .flatten()
    });
    if let Some(local) = local.as_ref() {
        validate_local_status(kind, local)?;
    }
    Ok(local)
}

/// Preserve foreign raw observations, replace this controller's stable key, and
/// prune only GatewayClass facts that prove an entry is obsolete.
pub fn merge_status(
    kind: &str,
    existing_status: Option<&Value>,
    local_status: &Value,
    identity: &ObserverIdentity,
    facts: &[ClassObservation],
) -> Result<Option<Value>> {
    if !is_partitioned_kind(kind) {
        return Ok(None);
    }

    let mut entries = match existing_status {
        None | Some(Value::Null) => Vec::new(),
        Some(status) if status.as_object().is_some_and(|object| object.is_empty()) => Vec::new(),
        Some(status)
            if !status
                .as_object()
                .is_some_and(|object| object.contains_key("controllers")) =>
        {
            retired_native_fields(kind, status)?;
            Vec::new()
        }
        Some(status) => parse_envelope(status)?,
    };
    entries.retain(|entry| !proven_obsolete(entry, facts));
    if entries.len() > MAX_CONTROLLER_OBSERVATIONS {
        bail!("controller status envelope exceeds {MAX_CONTROLLER_OBSERVATIONS} observations after pruning");
    }

    let replacement = json!({
        "controllerName": identity.controller_name,
        "gatewayClassName": identity.gateway_class_name,
        "gatewayClassUid": identity.gateway_class_uid,
        "status": local_status,
    });
    if let Some(index) = entries.iter().position(|entry| {
        entry_controller_name(entry) == identity.controller_name
            && entry_gateway_class_name(entry) == identity.gateway_class_name
    }) {
        entries[index] = replacement;
    } else {
        if entries.len() >= MAX_CONTROLLER_OBSERVATIONS {
            bail!("controller status envelope exceeds {MAX_CONTROLLER_OBSERVATIONS} observations");
        }
        entries.push(replacement);
    }
    Ok(Some(json!({"controllers": entries})))
}

/// Return recognized native fields that an outgoing Kubernetes patch must
/// tombstone after the envelope has replaced the old shape.
pub fn retired_native_fields(kind: &str, status: &Value) -> Result<Vec<String>> {
    if !is_partitioned_kind(kind) {
        return Ok(Vec::new());
    }
    let all_fields = || native_fields(kind).iter().map(|field| (*field).to_owned()).collect();
    if status.is_null() {
        return Ok(all_fields());
    }
    let object = status
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("native status must be an object"))?;
    if object.is_empty() {
        return Ok(all_fields());
    }
    if object.contains_key("controllers") {
        parse_envelope(status)?;
        return Ok(Vec::new());
    }
    let allowed = native_fields(kind);
    if object.keys().any(|field| !allowed.contains(&field.as_str())) {
        bail!("status is neither an empty, recognized native shape nor a controller envelope");
    }
    validate_local_status(kind, status)?;
    Ok(all_fields())
}

fn parse_envelope(status: &Value) -> Result<Vec<Value>> {
    let object = status
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("controller status must be an object envelope"))?;
    if object.len() != 1 || !object.contains_key("controllers") {
        bail!("controller status envelope cannot mix controllers with native status fields");
    }
    let entries = object["controllers"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("controller status envelope controllers must be an array"))?;
    let mut keys = HashSet::with_capacity(entries.len());
    for entry in entries {
        validate_entry(entry)?;
        let key = (
            entry_controller_name(entry).to_owned(),
            entry_gateway_class_name(entry).to_owned(),
        );
        if !keys.insert(key) {
            bail!("controller status envelope contains duplicate controllerName/gatewayClassName entries");
        }
    }
    Ok(entries.clone())
}

fn validate_entry(entry: &Value) -> Result<()> {
    let object = entry
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("controller status observation must be an object"))?;
    for field in ["controllerName", "gatewayClassName", "gatewayClassUid"] {
        if object.get(field).and_then(Value::as_str).is_none_or(str::is_empty) {
            bail!("controller status observation {field} must be a non-empty string");
        }
    }
    if !object.get("status").is_some_and(Value::is_object) {
        bail!("controller status observation status must be an object");
    }
    Ok(())
}

fn validate_local_status(kind: &str, status: &Value) -> Result<()> {
    match kind {
        "EdgionPlugins" | "EdgionStreamPlugins" | "EdgionBackend" => {
            serde_json::from_value::<ConditionsStatus>(status.clone())?;
        }
        "LinkSys" => {
            serde_json::from_value::<LinkSysLocalStatus>(status.clone())?;
        }
        _ => {}
    }
    Ok(())
}

fn native_fields(kind: &str) -> &'static [&'static str] {
    match kind {
        "EdgionPlugins" | "EdgionStreamPlugins" | "EdgionBackend" => &["conditions"],
        "LinkSys" => &[
            "conditions",
            "observedGeneration",
            "credentialState",
            "publishedGeneration",
            "issuedAt",
            "validUntil",
            "lastSuccessfulRefreshTime",
            "nextRefreshTime",
            "desiredConsumerCount",
            "publishedConsumerCount",
            "conflictedConsumerCount",
        ],
        _ => &[],
    }
}

fn proven_obsolete(entry: &Value, facts: &[ClassObservation]) -> bool {
    facts.iter().any(|fact| match fact {
        ClassObservation::Deleted { name } => entry_gateway_class_name(entry) == name,
        ClassObservation::Existing {
            name,
            uid,
            controller_name,
        } => {
            entry_gateway_class_name(entry) == name
                && (entry_gateway_class_uid(entry) != uid || entry_controller_name(entry) != controller_name)
        }
    })
}

fn entry_controller_name(entry: &Value) -> &str {
    entry["controllerName"]
        .as_str()
        .expect("validated controller observation")
}

fn entry_gateway_class_name(entry: &Value) -> &str {
    entry["gatewayClassName"]
        .as_str()
        .expect("validated controller observation")
}

fn entry_gateway_class_uid(entry: &Value) -> &str {
    entry["gatewayClassUid"]
        .as_str()
        .expect("validated controller observation")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> ObserverIdentity {
        ObserverIdentity {
            controller_name: "example/controller".into(),
            gateway_class_name: "edge".into(),
            gateway_class_uid: "uid-edge".into(),
        }
    }

    #[test]
    fn projects_exact_uid_without_flat_fallback() {
        let status = json!({"controllers": [{
            "controllerName": "example/controller", "gatewayClassName": "edge",
            "gatewayClassUid": "old-uid", "status": {"conditions": []}
        }]});
        assert_eq!(project_status("EdgionPlugins", &status, &identity()).unwrap(), None);
        assert_eq!(
            project_status("EdgionPlugins", &json!({"conditions": []}), &identity()).unwrap(),
            None
        );
        assert!(project_status("EdgionPlugins", &json!({"unexpected": true}), &identity()).is_err());
    }

    #[test]
    fn merge_preserves_order_and_foreign_entries() {
        let existing = json!({"controllers": [
            {"controllerName": "foreign", "gatewayClassName": "foreign-class", "gatewayClassUid": "uid-f", "status": {"value": 1}},
            {"controllerName": "example/controller", "gatewayClassName": "edge", "gatewayClassUid": "old", "status": {"value": 2}}
        ]});
        let merged = merge_status("EdgionPlugins", Some(&existing), &json!({"value": 3}), &identity(), &[])
            .unwrap()
            .unwrap();
        assert_eq!(merged["controllers"][0]["status"]["value"], 1);
        assert_eq!(merged["controllers"][1]["gatewayClassUid"], "uid-edge");
        assert_eq!(merged["controllers"][1]["status"]["value"], 3);
    }

    #[test]
    fn prunes_only_authoritative_evidence_then_applies_cap() {
        let mut entries: Vec<Value> = (0..MAX_CONTROLLER_OBSERVATIONS)
            .map(|index| {
                json!({"controllerName": "foreign", "gatewayClassName": format!("class-{index}"),
                "gatewayClassUid": format!("uid-{index}"), "status": {}})
            })
            .collect();
        let existing = json!({"controllers": entries});
        assert!(merge_status("EdgionPlugins", Some(&existing), &json!({}), &identity(), &[]).is_err());
        entries[0] = json!({
            "controllerName": "example/controller", "gatewayClassName": "edge",
            "gatewayClassUid": "old-uid", "status": {}
        });
        let existing_owner = json!({"controllers": entries});
        let updated = merge_status(
            "EdgionPlugins",
            Some(&existing_owner),
            &json!({"conditions": []}),
            &identity(),
            &[],
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            updated["controllers"].as_array().unwrap().len(),
            MAX_CONTROLLER_OBSERVATIONS
        );
        assert_eq!(updated["controllers"][0]["gatewayClassUid"], "uid-edge");
        entries.push(json!({
            "controllerName": "foreign", "gatewayClassName": "class-over-cap",
            "gatewayClassUid": "uid-over-cap", "status": {}
        }));
        let oversized_owner = json!({"controllers": entries});
        assert!(merge_status("EdgionPlugins", Some(&oversized_owner), &json!({}), &identity(), &[]).is_err());
        let facts = [ClassObservation::Deleted { name: "class-0".into() }];
        let merged = merge_status("EdgionPlugins", Some(&existing), &json!({}), &identity(), &facts)
            .unwrap()
            .unwrap();
        assert_eq!(
            merged["controllers"].as_array().unwrap().len(),
            MAX_CONTROLLER_OBSERVATIONS
        );
    }

    #[test]
    fn native_status_is_retired_but_never_projected() {
        let native = json!({"conditions": []});
        assert_eq!(retired_native_fields("EdgionPlugins", &native).unwrap(), ["conditions"]);
        assert_eq!(retired_native_fields("LinkSys", &json!({})).unwrap().len(), 11);
        assert_eq!(
            merge_status("EdgionPlugins", Some(&native), &json!({}), &identity(), &[])
                .unwrap()
                .unwrap(),
            json!({"controllers": [{
                "controllerName": "example/controller", "gatewayClassName": "edge",
                "gatewayClassUid": "uid-edge", "status": {}
            }]})
        );
    }
}
