//! Kubernetes status projection and merge for controller-partitioned resources.

use std::collections::HashSet;

use anyhow::{bail, Result};
use serde_json::{json, Value};

use edgion_resources::resources::common::ConditionsStatus;
use edgion_resources::resources::common::MAX_CONTROLLER_OBSERVATIONS;
use edgion_resources::resources::link_sys::LinkSysLocalStatus;

/// The deployment that owns a local observation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObserverIdentity {
    pub controller_name: String,
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
        (entry_controller_name(&entry) == identity.controller_name)
            .then(|| entry.get("status").cloned())
            .flatten()
    });
    if let Some(local) = local.as_ref() {
        validate_local_status(kind, local)?;
    }
    Ok(local)
}

/// Preserve every foreign observation and replace only this deployment's key.
pub fn merge_status(
    kind: &str,
    existing_status: Option<&Value>,
    local_status: &Value,
    identity: &ObserverIdentity,
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
    if entries.len() > MAX_CONTROLLER_OBSERVATIONS {
        bail!("controller status envelope exceeds {MAX_CONTROLLER_OBSERVATIONS} observations");
    }

    let replacement = json!({
        "controllerName": identity.controller_name,
        "status": local_status,
    });
    if let Some(index) = entries
        .iter()
        .position(|entry| entry_controller_name(entry) == identity.controller_name)
    {
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
        let key = entry_controller_name(entry);
        if !keys.insert(key) {
            bail!("controller status envelope contains duplicate controllerName entries");
        }
    }
    Ok(entries.clone())
}

fn validate_entry(entry: &Value) -> Result<()> {
    let object = entry
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("controller status observation must be an object"))?;
    if object
        .keys()
        .any(|field| !matches!(field.as_str(), "controllerName" | "status"))
    {
        bail!("controller status observation contains unsupported fields");
    }
    if object
        .get("controllerName")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        bail!("controller status observation controllerName must be a non-empty string");
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

fn entry_controller_name(entry: &Value) -> &str {
    entry["controllerName"]
        .as_str()
        .expect("validated controller observation")
}

#[cfg(test)]
#[path = "../../../../tests/unit/conf_mgr/sync_runtime/resource_processor/status_observation_test.rs"]
mod tests;
