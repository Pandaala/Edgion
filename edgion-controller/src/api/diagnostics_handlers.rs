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
//! * `GET /api/v1/diagnostics/unknown-fields` — fleet-wide list of
//!   `EdgionPlugins`, `EdgionStreamPlugins`, `EdgionConfigData`, and `LinkSys` resources
//!   whose submitted document contained a key the schema does not recognize. See
//!   `skills/01-architecture/01-controller/02-admin-api.md`. Backed by
//!   `observe::unknown_fields_registry`, not
//!   `center_list_resources`, because the finding lives in a
//!   `#[serde(skip)]` field invisible through the re-serialized listing path.
//!
//! All three are live-reads (`nested_ext_ref_usage` / `conf-conflicts`
//! against `ConfCenter`, `unknown-fields` against the typed `RawStore`
//! registry); no background tasks or snapshots.

use crate::api::types::AdminState;
use crate::api::ApiResponse;
use crate::observe::unknown_fields_registry::{collect_unknown_fields, UnknownFieldsEntry};
use axum::{extract::State, http::StatusCode, response::Json};
use edgion_resources::ResourceKind;
use serde::Serialize;
use std::sync::Arc;

/// Cap the number of example resources returned, to keep the payload small
/// even on clusters with a large fleet of legacy nested configurations.
const MAX_EXAMPLES: usize = 20;

/// Cap the number of unknown-fields findings returned per call, mirroring
/// `MAX_EXAMPLES`'s role for `nested_ext_ref_usage`. `total` always reports
/// the pre-cap count and `truncated` is set honestly rather than silently
/// dropping entries.
const MAX_UNKNOWN_FIELDS_ITEMS: usize = 100;

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

#[derive(Serialize)]
pub struct UnknownFieldsItem {
    pub kind: &'static str,
    pub namespace: String,
    pub name: String,
    pub paths: Vec<String>,
}

impl From<UnknownFieldsEntry> for UnknownFieldsItem {
    fn from(e: UnknownFieldsEntry) -> Self {
        Self {
            kind: e.kind,
            namespace: e.namespace,
            name: e.name,
            paths: e.paths,
        }
    }
}

#[derive(Serialize)]
pub struct UnknownFieldsResponse {
    /// Findings across all resources, BEFORE the `MAX_UNKNOWN_FIELDS_ITEMS`
    /// cap is applied.
    pub total: usize,
    pub truncated: bool,
    pub items: Vec<UnknownFieldsItem>,
}

/// `GET /api/v1/diagnostics/unknown-fields`.
///
/// Fleet-wide answer to "did anything applied to this controller contain a
/// key the schema does not recognize?", covering `EdgionPlugins`,
/// `EdgionStreamPlugins`, `EdgionConfigData`, and `LinkSys`. Never rejects
/// anything itself (warn-not-reject) — this is the query surface an
/// operator's own blocking gate (CI, a ValidatingAdmissionWebhook) is meant to
/// be built on.
pub async fn unknown_fields(State(_state): State<Arc<AdminState>>) -> Json<ApiResponse<UnknownFieldsResponse>> {
    let (total, truncated, items) = collect_unknown_fields(MAX_UNKNOWN_FIELDS_ITEMS);
    Json(ApiResponse::ok_body(UnknownFieldsResponse {
        total,
        truncated,
        items: items.into_iter().map(UnknownFieldsItem::from).collect(),
    }))
}

#[cfg(test)]
#[path = "../../tests/unit/api/diagnostics_handlers_test.rs"]
mod tests;
