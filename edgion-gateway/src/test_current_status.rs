//! Processed-resource fixtures with explicit Controller-owned status projections.

use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};

/// Deserialize a fixture representing a completed Controller publication.
/// Raw-input and missing-projection tests must use ordinary serde instead.
pub(crate) fn from_value<T: DeserializeOwned>(mut value: Value) -> serde_json::Result<T> {
    let kind = value.get("kind").and_then(Value::as_str).unwrap_or_else(|| {
        if value["spec"].get("resolvedStatusController").is_some() {
            "HTTPRoute"
        } else if value["spec"].get("gatewayClassName").is_some() {
            "Gateway"
        } else {
            ""
        }
    });
    let current = match kind {
        "HTTPRoute" | "GRPCRoute" | "TCPRoute" | "UDPRoute" | "TLSRoute" => {
            let owner = value["spec"]["resolvedStatusController"]
                .as_str()
                .unwrap_or("edgion.io/gateway-controller");
            let parents: Vec<_> = value["spec"]["parentRefs"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|parent| {
                    json!({
                        "parentRef": parent,
                        "controllerName": owner,
                        "conditions": [accepted_condition()]
                    })
                })
                .collect();
            Some(json!({"parents": parents}))
        }
        "Gateway" => {
            let listeners: Vec<_> = value["spec"]["listeners"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|listener| {
                    json!({
                        "name": listener["name"], "supportedKinds": [], "attachedRoutes": 0,
                        "conditions": [accepted_condition()]
                    })
                })
                .collect();
            Some(json!({"listeners": listeners}))
        }
        "EdgionAcme" => Some(json!({})),
        _ => None,
    };
    if let Some(current) = current {
        if value["spec"].get("currentStatus").is_none() {
            value["spec"]["currentStatus"] = value
                .get("status")
                .filter(|status| !status.is_null())
                .cloned()
                .unwrap_or(current);
        }
    }
    serde_json::from_value(value)
}

/// Complete a typed fixture at its construction boundary, not at consumption.
pub(crate) fn processed<T: Serialize + DeserializeOwned>(value: T) -> T {
    from_value(serde_json::to_value(value).expect("fixture serializes")).expect("processed fixture deserializes")
}

fn accepted_condition() -> Value {
    json!({"type": "Accepted", "status": "True", "reason": "Accepted", "message": "",
        "lastTransitionTime": "2026-09-21T00:00:00Z"})
}
