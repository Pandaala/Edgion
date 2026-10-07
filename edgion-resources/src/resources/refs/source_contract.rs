//! Finite reference validation from exact, code-owned Gateway API source GVKs.
//!
//! This is a scoped validation view, not a Kubernetes admission replacement.
use crate::crd_contract::{check_input_bounds, CompiledFragment, CrdContractError, CrdViolation};
use serde::Deserialize;
use serde_json::Value;
use std::sync::LazyLock;

#[derive(Deserialize)]
struct Document {
    contracts: Vec<Source>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Source {
    group: String,
    version: String,
    kind: String,
    channel: String,
    gateway_api_version: String,
    artifact: String,
    selected_sha256: String,
    schema: Value,
}

struct Contract {
    source: Source,
    fragment: CompiledFragment,
}

static CONTRACTS: LazyLock<Result<Vec<Contract>, CrdContractError>> = LazyLock::new(|| {
    let document: Document = serde_json::from_str(include_str!("../../contracts/gateway_api_references.json"))
        .map_err(|_| CrdContractError::InvalidContract)?;
    if document.contracts.len() != 18 {
        return Err(CrdContractError::InvalidContract);
    }
    document
        .contracts
        .into_iter()
        .map(|source| {
            if source.group != "gateway.networking.k8s.io"
                || source.selected_sha256.len() != 64
                || source.artifact.is_empty()
                || !matches!(source.channel.as_str(), "standard" | "experimental" | "historicalGrpc")
                || !matches!(source.gateway_api_version.as_str(), "v1.6.2" | "v1.1.1")
            {
                return Err(CrdContractError::InvalidContract);
            }
            let fragment = CompiledFragment::compile(source.schema.clone(), "/referenceContract")?;
            Ok(Contract { source, fragment })
        })
        .collect()
});

/// On success, the resource with only selected reference fields normalized.
/// A failure retains only its bounded projection for safe diagnostics.
/// The submitted resource itself is never modified.
pub struct GatewayReferenceView {
    pub normalized: Value,
    pub violations: Vec<CrdViolation>,
}

/// Select the repository's exact source GVK profile; callers cannot choose a channel.
/// Unregistered GVKs remain the existing source registry's responsibility.
pub fn prepare_gateway_source(
    group: &str,
    version: &str,
    kind: &str,
    authored: &Value,
) -> Result<Option<GatewayReferenceView>, CrdContractError> {
    let contracts = CONTRACTS.as_ref().map_err(|_| CrdContractError::InvalidContract)?;
    let Some(contract) = contracts.iter().find(|contract| {
        contract.source.group == group && contract.source.version == version && contract.source.kind == kind
    }) else {
        return Ok(None);
    };
    let projection = project(&contract.source.schema, authored)?;
    let (projection, violations) = contract.fragment.prepare(&projection)?;
    if !violations.is_empty() {
        return Ok(Some(GatewayReferenceView {
            normalized: projection,
            violations,
        }));
    }
    let mut normalized = authored.clone();
    merge_normalized(&contract.source.schema, &projection, &mut normalized);
    Ok(Some(GatewayReferenceView { normalized, violations }))
}

// Project only selected structural properties. Complete leaf schemas (including
// options maps and reference scalar fields) retain their actual submitted values.
fn project(schema: &Value, value: &Value) -> Result<Value, CrdContractError> {
    if let (Some(properties), Some(object)) = (schema.get("properties").and_then(Value::as_object), value.as_object()) {
        let mut projected = serde_json::Map::new();
        for (key, child) in properties {
            if let Some(value) = object.get(key) {
                projected.insert(key.clone(), project(child, value)?);
            }
        }
        Ok(Value::Object(projected))
    } else if let (Some(items), Some(values)) = (schema.get("items"), value.as_array()) {
        // The raw array bound is reference-scoped, independent of unrelated content.
        if schema
            .get("maxItems")
            .and_then(Value::as_u64)
            .is_some_and(|max| values.len() as u64 > max)
        {
            // Preserve a bounded invalid list for the schema verdict without
            // cloning unbounded submitted children.
            let max = schema.get("maxItems").and_then(Value::as_u64).unwrap_or_default() as usize;
            return values
                .iter()
                .take(max + 1)
                .map(|value| project(items, value))
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array);
        }
        values
            .iter()
            .map(|value| project(items, value))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array)
    } else {
        check_input_bounds(value)?;
        Ok(value.clone())
    }
}

fn merge_normalized(schema: &Value, projection: &Value, output: &mut Value) {
    if let (Some(properties), Some(values), Some(object)) = (
        schema.get("properties").and_then(Value::as_object),
        projection.as_object(),
        output.as_object_mut(),
    ) {
        for (key, child_schema) in properties {
            if let Some(value) = values.get(key) {
                if let Some(current) = object.get_mut(key) {
                    merge_normalized(child_schema, value, current);
                } else {
                    object.insert(key.clone(), value.clone());
                }
            } else {
                object.remove(key);
            }
        }
    } else if let (Some(items), Some(values), Some(outputs)) =
        (schema.get("items"), projection.as_array(), output.as_array_mut())
    {
        for (value, output) in values.iter().zip(outputs) {
            merge_normalized(items, value, output);
        }
    } else {
        *output = projection.clone();
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/refs/source_contract_test.rs"]
mod tests;
