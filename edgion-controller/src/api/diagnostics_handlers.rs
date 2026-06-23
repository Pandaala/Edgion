//! Diagnostics endpoints for operational inspection.
//!
//! * `GET /api/v1/diagnostics/nested_ext_ref_usage` — counts `EdgionPlugins`
//!   resources that still embed `type: ExtensionRef` inside a stage list
//!   (`requestPlugins` / `upstreamResponseFilterPlugins` /
//!   `upstreamResponseBodyFilterPlugins` / `upstreamResponsePlugins`).
//!
//! * `GET /api/v1/diagnostics/conf-conflicts` — reports `(kind, key)` collisions
//!   from the FileSystem backend's recursive scan.
//!
//! Both are live-reads against `ConfCenter`; no background tasks or snapshots.

use crate::api::types::AdminState;
use crate::api::ApiResponse;
use axum::{extract::State, http::StatusCode, response::Json};
use edgion_resources::ResourceKind;
use serde::Serialize;
use std::sync::Arc;

/// Cap the number of example resources returned, to keep the payload small
/// even on clusters with a large fleet of legacy nested configurations.
const MAX_EXAMPLES: usize = 20;

#[derive(Serialize)]
pub struct NestedExtRefUsageStats {
    pub total_edgion_plugins: usize,
    pub with_nested_ext_ref: usize,
    pub examples: Vec<NestedExtRefExample>,
}

#[derive(Serialize)]
pub struct NestedExtRefExample {
    pub namespace: String,
    pub name: String,
    /// One of `request` / `upstreamResponseFilter` / `upstreamResponseBodyFilter`
    /// / `upstreamResponse`. Always the first stage in which a nested
    /// ExtensionRef is observed; later stages on the same resource are not
    /// reported separately to keep examples one-per-resource.
    pub stage: &'static str,
}

/// `GET /api/v1/diagnostics/nested_ext_ref_usage`.
pub async fn nested_ext_ref_usage(
    State(state): State<Arc<AdminState>>,
) -> Result<Json<ApiResponse<NestedExtRefUsageStats>>, StatusCode> {
    let (items, _continue) = state.center_list_resources(ResourceKind::EdgionPlugins, None).await?;

    let total_edgion_plugins = items.len();
    let mut with_nested_ext_ref = 0usize;
    let mut examples: Vec<NestedExtRefExample> = Vec::new();

    for item in &items {
        let Some(stage) = first_nested_ext_ref_stage(item) else {
            continue;
        };
        with_nested_ext_ref += 1;
        if examples.len() < MAX_EXAMPLES {
            let (namespace, name) = extract_identity(item);
            examples.push(NestedExtRefExample { namespace, name, stage });
        }
    }

    Ok(Json(ApiResponse::ok_body(NestedExtRefUsageStats {
        total_edgion_plugins,
        with_nested_ext_ref,
        examples,
    })))
}

/// Return the first stage in which an `EdgionPlugins` resource's spec embeds
/// a `type: ExtensionRef` entry, or `None` if no nested ExtensionRef is found.
///
/// Stage scan order is fixed (`request` → `upstreamResponseFilter` →
/// `upstreamResponseBodyFilter` → `upstreamResponse`) so the example is
/// stable across calls.
fn first_nested_ext_ref_stage(item: &serde_json::Value) -> Option<&'static str> {
    let spec = item.get("spec")?;

    const STAGES: &[(&str, &str)] = &[
        ("requestPlugins", "request"),
        ("upstreamResponseFilterPlugins", "upstreamResponseFilter"),
        ("upstreamResponseBodyFilterPlugins", "upstreamResponseBodyFilter"),
        ("upstreamResponsePlugins", "upstreamResponse"),
    ];

    for (field, stage) in STAGES {
        if let Some(arr) = spec.get(*field).and_then(|v| v.as_array()) {
            if arr.iter().any(entry_is_extension_ref) {
                return Some(stage);
            }
        }
    }
    None
}

/// `RequestFilterEntry` and friends `#[serde(flatten)]` an `EdgionPlugin` into
/// the entry object, so a nested ExtensionRef shows up as a top-level
/// `type: ExtensionRef` on the entry itself.
fn entry_is_extension_ref(entry: &serde_json::Value) -> bool {
    entry
        .get("type")
        .and_then(|v| v.as_str())
        .is_some_and(|s| s == "ExtensionRef")
}

fn extract_identity(item: &serde_json::Value) -> (String, String) {
    let meta = item.get("metadata");
    let namespace = meta
        .and_then(|m| m.get("namespace"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let name = meta
        .and_then(|m| m.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    (namespace, name)
}

#[derive(Serialize)]
pub struct ConfConflictItem {
    pub kind: String,
    pub key: String,
    pub winner: String,
    pub losers: Vec<String>,
}

#[derive(Serialize)]
pub struct ConfConflictsResponse {
    pub conflicts: Vec<ConfConflictItem>,
}

/// `GET /api/v1/diagnostics/conf-conflicts`.
///
/// Surfaces `(kind, key)` collisions where the same logical resource is
/// recorded at more than one path under `--conf-dir` after the FileSystem
/// backend's recursive scan. Returns an empty list when no conflicts exist
/// or when running in a non-FileSystem mode.
pub async fn conf_conflicts(State(state): State<Arc<AdminState>>) -> Json<ApiResponse<ConfConflictsResponse>> {
    let conflicts = match state.conf_dir_registry() {
        Some(reg) => reg
            .snapshot_conflicts()
            .into_iter()
            .map(|c| ConfConflictItem {
                kind: c.key.kind,
                key: c.key.key,
                winner: c.winner.display().to_string(),
                losers: c.losers.into_iter().map(|p| p.display().to_string()).collect(),
            })
            .collect(),
        None => Vec::new(),
    };
    Json(ApiResponse::ok_body(ConfConflictsResponse { conflicts }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ep(ns: &str, name: &str, spec: serde_json::Value) -> serde_json::Value {
        json!({
            "metadata": { "namespace": ns, "name": name },
            "spec": spec,
        })
    }

    #[test]
    fn no_nested_ext_ref_returns_none() {
        let v = ep(
            "default",
            "a",
            json!({
                "requestPlugins": [
                    { "type": "BasicAuth", "config": { "users": [] } }
                ]
            }),
        );
        assert!(first_nested_ext_ref_stage(&v).is_none());
    }

    #[test]
    fn nested_ext_ref_in_request_detected() {
        let v = ep(
            "default",
            "a",
            json!({
                "requestPlugins": [
                    { "type": "ExtensionRef", "config": { "name": "B" } }
                ]
            }),
        );
        assert_eq!(first_nested_ext_ref_stage(&v), Some("request"));
    }

    #[test]
    fn nested_ext_ref_in_body_filter_detected() {
        let v = ep(
            "default",
            "a",
            json!({
                "upstreamResponseBodyFilterPlugins": [
                    { "type": "ExtensionRef", "config": { "name": "B" } }
                ]
            }),
        );
        assert_eq!(first_nested_ext_ref_stage(&v), Some("upstreamResponseBodyFilter"));
    }

    #[test]
    fn first_stage_wins_when_multiple_present() {
        let v = ep(
            "default",
            "a",
            json!({
                "requestPlugins": [
                    { "type": "ExtensionRef", "config": { "name": "B" } }
                ],
                "upstreamResponsePlugins": [
                    { "type": "ExtensionRef", "config": { "name": "C" } }
                ]
            }),
        );
        assert_eq!(first_nested_ext_ref_stage(&v), Some("request"));
    }

    #[test]
    fn missing_spec_is_safe() {
        let v = json!({ "metadata": { "namespace": "default", "name": "a" } });
        assert!(first_nested_ext_ref_stage(&v).is_none());
    }

    #[test]
    fn extension_ref_at_other_levels_ignored() {
        // A route-level filter (HTTPRoute.rules[].filters) would carry
        // `type: ExtensionRef`, but lives in a different resource entirely;
        // we only scan `EdgionPlugins.spec.*Plugins` so it cannot show up
        // here. Sanity check: an unrelated nested object containing
        // `type: ExtensionRef` outside the scanned arrays is ignored.
        let v = ep(
            "default",
            "a",
            json!({
                "someOtherField": [
                    { "type": "ExtensionRef" }
                ],
                "requestPlugins": [
                    { "type": "BasicAuth", "config": {} }
                ]
            }),
        );
        assert!(first_nested_ext_ref_stage(&v).is_none());
    }

    #[test]
    fn extract_identity_handles_missing_metadata() {
        let v = json!({});
        let (ns, name) = extract_identity(&v);
        assert_eq!(ns, "");
        assert_eq!(name, "");
    }
}
