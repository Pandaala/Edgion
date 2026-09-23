use anyhow::{Context, Result};
use serde_json::Value;

/// Return the selected controller's resource conditions in either test topology.
pub fn conditions(resource: &Value) -> Result<&Vec<Value>> {
    let status = if std::env::var("EDGION_TEST_K8S_MODE").as_deref() == Ok("true") {
        resource["status"]["controllers"]
            .as_array()
            .and_then(|controllers| {
                controllers
                    .iter()
                    .find(|controller| controller["controllerName"] == "edgion.io/gateway-controller")
            })
            .map(|controller| &controller["status"])
            .context("missing Kubernetes controller status")?
    } else {
        &resource["status"]
    };
    status["conditions"].as_array().context("missing status conditions")
}
