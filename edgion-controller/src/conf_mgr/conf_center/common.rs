//! Common configuration types shared across all configuration center backends
//!
//! This module contains types that are used by both FileSystem and Kubernetes modes.

use edgion_common::time::now_rfc3339;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::OnceLock;

use crate::conf_mgr::conf_center::traits::ConfWriterError;

use edgion_resources::resources::common::Condition;

// ============================================================================
// Shared controller constants and helpers (FileSystem and Etcd backends)
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeInvalidEnvelope {
    pub reason: &'static str,
    pub message: String,
}

pub type RuntimeInvalidEnvelopes = RwLock<HashMap<String, RuntimeInvalidEnvelope>>;

pub fn store_runtime_invalid(envelopes: &RuntimeInvalidEnvelopes, key: String, reason: &'static str, message: String) {
    envelopes
        .write()
        .insert(key, RuntimeInvalidEnvelope { reason, message });
}

pub fn clear_runtime_invalid(envelopes: &RuntimeInvalidEnvelopes, key: &str) {
    envelopes.write().remove(key);
}

pub fn get_runtime_invalid(envelopes: &RuntimeInvalidEnvelopes, key: &str) -> Option<RuntimeInvalidEnvelope> {
    envelopes.read().get(key).cloned()
}

// ============================================================================
// Shared status types (used by both FileSystem and Etcd backends)
// ============================================================================

/// Condition type used exclusively by [`ErrorStatus`] (parse-failure fallback).
///
/// No resource handler manages a condition of this type; its presence in a
/// persisted status marks the status as an ErrorStatus sidecar (see
/// `seed_existing_status`, which must not seed it back into a recovered
/// resource).
pub const ERROR_STATUS_CONDITION_TYPE: &str = "Ready";

/// Return whether a persisted status document is an [`ErrorStatus`] sidecar.
///
/// The sole marker is [`ERROR_STATUS_CONDITION_TYPE`]: no resource handler
/// manages a condition of that type, so its presence proves the document was
/// written by `write_error_status` rather than computed from the resource.
/// Both the status seed and the post-repair sidecar cleanup must agree on this
/// predicate, so it has exactly one implementation.
pub fn is_error_status_sidecar(status: &Value) -> bool {
    status
        .get("conditions")
        .and_then(|conditions| conditions.as_array())
        .is_some_and(|conditions| {
            conditions.iter().any(|condition| {
                condition.get("type").and_then(|kind| kind.as_str()) == Some(ERROR_STATUS_CONDITION_TYPE)
            })
        })
}

/// [`is_error_status_sidecar`] over the persisted JSON text; unparsable status
/// is never treated as a sidecar.
pub fn is_error_status_sidecar_json(status_json: &str) -> bool {
    serde_json::from_str::<Value>(status_json).is_ok_and(|status| is_error_status_sidecar(&status))
}

/// Simple error status structure (for parse errors, etc.)
///
/// Used when the resource can't be fully parsed and we can't extract native status.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorStatus {
    /// Standard K8s conditions
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<Condition>,
}

impl ErrorStatus {
    /// Create an error status
    pub fn error(reason: &str, message: &str) -> Self {
        Self {
            conditions: vec![Condition {
                type_: ERROR_STATUS_CONDITION_TYPE.to_string(),
                status: "False".to_string(),
                reason: reason.to_string(),
                message: message.to_string(),
                last_transition_time: now_rfc3339(),
                observed_generation: None,
            }],
        }
    }

    /// Build the error status to persist, compared against the previously
    /// persisted status JSON (if any).
    ///
    /// Returns `None` when the stored status already carries a semantically
    /// identical error condition (same type/status/reason/message) — the
    /// caller must skip the write so the stored `lastTransitionTime` stays
    /// stable across re-scans of the same broken resource. When the condition
    /// differs but its `status` did not flip, the stored `lastTransitionTime`
    /// is carried over (transition-only LTT contract).
    pub fn error_for_persist(reason: &str, message: &str, existing_json: Option<&str>) -> Option<Self> {
        let mut status = Self::error(reason, message);
        let Some(existing) = existing_json.and_then(|j| serde_json::from_str::<serde_json::Value>(j).ok()) else {
            return Some(status);
        };
        let new_cond = &mut status.conditions[0];
        let old_cond = existing.get("conditions").and_then(|c| c.as_array()).and_then(|arr| {
            arr.iter()
                .find(|c| c.get("type").and_then(|t| t.as_str()) == Some(&new_cond.type_))
        });
        let Some(old_cond) = old_cond else {
            return Some(status);
        };
        let field = |name: &str| old_cond.get(name).and_then(|v| v.as_str()).unwrap_or("");
        if field("status") == new_cond.status {
            // No transition: keep the stored lastTransitionTime.
            let old_ltt = field("lastTransitionTime");
            if !old_ltt.is_empty() {
                new_cond.last_transition_time = old_ltt.to_string();
            }
            if field("reason") == new_cond.reason && field("message") == new_cond.message {
                return None;
            }
        }
        Some(status)
    }
}

// ============================================================================
// Shared resource parsing (used by both FileSystem and Etcd backends)
// ============================================================================

use edgion_resources::{ResourceKind, ResourceMeta};
use kube::Resource;
use serde::de::{DeserializeOwned, Error as _};
use std::fmt::Debug;

/// Check a raw Gateway API document before typed serde can discard TypeMeta.
/// Only explicitly inventoried source versions are admitted.
pub fn validate_gateway_api_source(kind: ResourceKind, value: &yaml_serde::Value) -> Result<(), String> {
    use edgion_resources::gateway_api_support::{canonical_sources, source_for, GROUP};

    let mut sources = canonical_sources().filter(|source| source.kind == kind.as_str());
    let Some(source) = sources.next() else {
        return Ok(());
    };
    if sources.next().is_some() {
        return Err(format!("{} has ambiguous canonical source versions", kind.as_str()));
    }

    let actual_kind = value.get("kind").and_then(yaml_serde::Value::as_str);
    if actual_kind != Some(source.kind) {
        return Err(format!("{} kind must be {}", kind.as_str(), source.kind));
    }
    let actual_version = value
        .get("apiVersion")
        .and_then(yaml_serde::Value::as_str)
        .and_then(|api_version| api_version.strip_prefix(&format!("{GROUP}/")));
    if actual_version.is_none_or(|version| source_for(source.kind, version).is_none()) {
        return Err(format!(
            "{} apiVersion must use a supported {GROUP} source version",
            kind.as_str()
        ));
    }
    Ok(())
}

/// Kubernetes CRUD checks these explicit source versions before v1 projection.
pub fn validate_backend_tls_policy_api_version(api_version: Option<&str>) -> Result<(), String> {
    validate_registered_source_version("BackendTLSPolicy", api_version)
}

pub fn validate_tls_route_api_version(api_version: Option<&str>) -> Result<(), String> {
    validate_registered_source_version("TLSRoute", api_version)
}

fn validate_registered_source_version(kind: &str, api_version: Option<&str>) -> Result<(), String> {
    use edgion_resources::gateway_api_support::{source_for, GROUP};
    if api_version
        .and_then(|api_version| api_version.strip_prefix(&format!("{GROUP}/")))
        .is_some_and(|version| source_for(kind, version).is_some())
    {
        Ok(())
    } else {
        Err(format!("{kind} apiVersion must use a supported {GROUP} source version"))
    }
}

/// Parse YAML content into a resource.
///
/// Gateway API sources are checked against the canonical inventory first.
pub fn parse_resource_content<K>(content: &str) -> yaml_serde::Result<K>
where
    K: ResourceMeta + Resource + Clone + Send + Sync + Debug + serde::Serialize + DeserializeOwned + 'static,
{
    parse_resource_content_inner(content, false)
}

/// Static backends validate selected alpha sources before canonical conversion.
pub fn parse_resource_content_offline<K>(content: &str) -> yaml_serde::Result<K>
where
    K: ResourceMeta + Resource + Clone + Send + Sync + Debug + serde::Serialize + DeserializeOwned + 'static,
{
    let resource: K = parse_resource_content_inner(content, true)?;
    let kind = K::resource_kind();
    // Gateway dependency requeues must identify the same source as the file or etcd key.
    if matches!(
        kind,
        ResourceKind::GRPCRoute | ResourceKind::TCPRoute | ResourceKind::UDPRoute
    ) && resource.meta().namespace.as_deref().unwrap_or("").is_empty()
    {
        return Err(yaml_serde::Error::custom(format!(
            "{} requires an explicit metadata.namespace",
            kind.as_str()
        )));
    }
    Ok(resource)
}

fn parse_resource_content_inner<K>(content: &str, validate_alpha_schema: bool) -> yaml_serde::Result<K>
where
    K: ResourceMeta + Resource + Clone + Send + Sync + Debug + serde::Serialize + DeserializeOwned + 'static,
{
    let kind = K::resource_kind();

    let raw: yaml_serde::Value = yaml_serde::from_str(content)?;
    validate_gateway_api_source(kind, &raw).map_err(yaml_serde::Error::custom)?;
    let source_version = raw.get("apiVersion").and_then(yaml_serde::Value::as_str);
    let beta_source = source_version == Some("gateway.networking.k8s.io/v1beta1");

    if matches!(
        kind,
        ResourceKind::GRPCRoute
            | ResourceKind::TLSRoute
            | ResourceKind::BackendTLSPolicy
            | ResourceKind::TCPRoute
            | ResourceKind::UDPRoute
    ) && source_version != Some("gateway.networking.k8s.io/v1")
    {
        let mut source = raw;
        if validate_alpha_schema {
            let version = source
                .get("apiVersion")
                .and_then(yaml_serde::Value::as_str)
                .and_then(|value| value.strip_prefix("gateway.networking.k8s.io/"))
                .ok_or_else(|| yaml_serde::Error::custom("Missing Gateway API source version"))?;
            static SOURCE_SCHEMAS: OnceLock<Result<crate::conf_mgr::SchemaValidator, String>> = OnceLock::new();
            let schemas = SOURCE_SCHEMAS.get_or_init(|| {
                let crd_dir = edgion_resources::work_dir().resolve("config/crd");
                #[cfg(test)]
                let crd_dir = if crd_dir.is_dir() {
                    crd_dir
                } else {
                    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../config/crd")
                };
                crate::conf_mgr::SchemaValidator::from_crd_dir(&crd_dir).map_err(|error| error.to_string())
            });
            let schemas = schemas
                .as_ref()
                .map_err(|error| yaml_serde::Error::custom(error.as_str()))?;
            let source_json = serde_json::to_value(&source).map_err(yaml_serde::Error::custom)?;
            schemas
                .validate_gateway_source("gateway.networking.k8s.io", version, kind.as_str(), &source_json)
                .map_err(yaml_serde::Error::custom)?;
        }
        source["apiVersion"] = yaml_serde::Value::String("gateway.networking.k8s.io/v1".to_string());
        return yaml_serde::from_value(source);
    }

    // Drop the raw view before text decoding unless advisory diagnostics need it.
    let diagnostic_raw = matches!(
        kind,
        ResourceKind::EdgionPlugins
            | ResourceKind::EdgionStreamPlugins
            | ResourceKind::EdgionConfigData
            | ResourceKind::LinkSys
    )
    .then_some(raw);
    let mut parsed = if kind == ResourceKind::Gateway && beta_source {
        let beta: edgion_resources::GatewayBeta = yaml_serde::from_str(content)?;
        let canonical: edgion_resources::Gateway = beta.into();
        yaml_serde::from_value(yaml_serde::to_value(canonical)?)?
    } else if kind == ResourceKind::HTTPRoute && beta_source {
        let beta: edgion_resources::HTTPRouteBeta = yaml_serde::from_str(content)?;
        let canonical: edgion_resources::HTTPRoute = beta.into();
        yaml_serde::from_value(yaml_serde::to_value(canonical)?)?
    } else if kind == ResourceKind::GatewayClass && beta_source {
        let beta: edgion_resources::GatewayClassBeta = yaml_serde::from_str(content)?;
        let canonical: edgion_resources::GatewayClass = beta.into();
        yaml_serde::from_value(yaml_serde::to_value(canonical)?)?
    } else if kind == ResourceKind::ReferenceGrant && beta_source {
        let beta: edgion_resources::ReferenceGrantBeta = yaml_serde::from_str(content)?;
        let canonical: edgion_resources::ReferenceGrant = beta.into();
        yaml_serde::from_value(yaml_serde::to_value(canonical)?)?
    } else {
        yaml_serde::from_str::<K>(content)?
    };

    // FileSystem/etcd unknown-field diagnostic: the Kubernetes watcher
    // gets this via the hand-written `Deserialize` impls on `EdgionPluginsSpec`,
    // `EdgionStreamPluginsSpec`, `EdgionConfigDataSpec`, and `LinkSysSpec`, gated by
    // `strict_ingest_enabled`. That gate means
    // "Kubernetes ingress" specifically (see its doc comment), so it is NOT
    // consulted here — FileSystem/etcd content is checked unconditionally.
    if let Some(raw) = diagnostic_raw {
        populate_unknown_fields_non_fatal(kind, &raw, &mut parsed);
    }

    Ok(parsed)
}

/// Validate API-version contracts that must be enforced before typed
/// deserialization can discard the submitted TypeMeta.
///
/// This is shared by FileSystem/etcd ingestion and the Admin create, update,
/// and validate endpoints so every ingress applies the same atomic API
/// promotion boundary.
pub fn validate_resource_api_version<K>(content: &str) -> yaml_serde::Result<()>
where
    K: ResourceMeta,
{
    if edgion_resources::gateway_api_support::canonical_sources()
        .any(|source| source.kind == K::resource_kind().as_str())
    {
        let value: yaml_serde::Value = yaml_serde::from_str(content)?;
        validate_gateway_api_source(K::resource_kind(), &value).map_err(yaml_serde::Error::custom)?;
    }

    Ok(())
}

#[cfg(test)]
#[path = "../../../tests/unit/conf_mgr/conf_center/common_test/gateway_api_version_guard_tests.rs"]
mod gateway_api_version_guard_tests;

/// Populate `spec.unknown_fields` on an already-parsed resource supported by
/// the unknown-field diagnostic, for the FileSystem/etcd loader seam.
///
/// Non-fatal by design: unlike the Kubernetes hook, this
/// borrows a raw YAML view that may contain shapes `serde_json::Value` cannot
/// represent (a non-string map key, `!!binary`, a YAML tag). This diagnostic
/// must never turn an otherwise-loadable document into a load failure, so any
/// conversion error here just skips the check and the caller's typed
/// parse (already completed above) is returned unchanged.
///
/// `K` is generic over every resource kind `parse_resource_content` serves,
/// so `Any::downcast_mut` is the minimal way to reach the concrete
/// supported spec fields without adding a trait method to every resource type
/// for a diagnostic only four kinds need. The
/// `K: 'static` bound `parse_resource_content` already carries makes this
/// sound.
fn populate_unknown_fields_non_fatal<K: 'static>(kind: ResourceKind, yaml_value: &yaml_serde::Value, parsed: &mut K) {
    let json_value: Value = match serde_json::to_value(yaml_value) {
        Ok(v) => v,
        Err(err) => {
            tracing::warn!(
                component = "conf_center",
                event = "unknown_fields_json_convert_skip",
                kind = ?kind,
                error = %err,
                "skipping unknown-field check: YAML document does not convert to JSON"
            );
            return;
        }
    };
    let Some(spec_value) = json_value.get("spec") else {
        return;
    };

    match kind {
        ResourceKind::EdgionPlugins => {
            if let Some(plugins) = (parsed as &mut dyn std::any::Any).downcast_mut::<edgion_resources::EdgionPlugins>()
            {
                plugins.spec.unknown_fields = edgion_resources::schema_check::unknown_key_paths(
                    spec_value,
                    edgion_resources::edgion_plugins_spec_schema(),
                );
            }
        }
        ResourceKind::EdgionStreamPlugins => {
            if let Some(plugins) =
                (parsed as &mut dyn std::any::Any).downcast_mut::<edgion_resources::EdgionStreamPlugins>()
            {
                plugins.spec.unknown_fields = edgion_resources::schema_check::unknown_key_paths(
                    spec_value,
                    edgion_resources::edgion_stream_plugins_spec_schema(),
                );
            }
        }
        ResourceKind::EdgionConfigData => {
            if let Some(config_data) =
                (parsed as &mut dyn std::any::Any).downcast_mut::<edgion_resources::EdgionConfigData>()
            {
                config_data.spec.unknown_fields = edgion_resources::schema_check::unknown_key_paths(
                    spec_value,
                    edgion_resources::edgion_config_data_spec_schema(),
                );
            }
        }
        ResourceKind::LinkSys => {
            if let Some(link_sys) = (parsed as &mut dyn std::any::Any).downcast_mut::<edgion_resources::LinkSys>() {
                link_sys.spec.unknown_fields = edgion_resources::schema_check::unknown_key_paths(
                    spec_value,
                    edgion_resources::link_sys_spec_schema(),
                );
            }
        }
        _ => {}
    }
}

/// Merge an operator update with the server-owned portions of the current resource.
///
/// The request is authoritative for the resource body, including `spec` and
/// top-level operator fields such as ConfigMap `data`. Only labels and
/// annotations are mutable inside metadata. Identity, all other current
/// metadata (including Kubernetes lifecycle fields), and status remain
/// server-owned. Starting from the request is important: controller-resolved
/// fields that happen to live below the current `spec` must never be copied
/// back into an operator update.
pub(crate) fn merge_operator_update_values(current: Value, requested: Value) -> Result<Value, ConfWriterError> {
    let current_obj = current
        .as_object()
        .ok_or_else(|| ConfWriterError::ValidationError("current resource must be an object".to_string()))?;
    let requested_obj = requested
        .as_object()
        .ok_or_else(|| ConfWriterError::ValidationError("updated resource must be an object".to_string()))?;

    let current_meta = current_obj
        .get("metadata")
        .and_then(Value::as_object)
        .ok_or_else(|| ConfWriterError::ValidationError("current resource metadata must be an object".to_string()))?;
    let requested_meta = requested_obj
        .get("metadata")
        .and_then(Value::as_object)
        .ok_or_else(|| ConfWriterError::ValidationError("updated resource metadata must be an object".to_string()))?;

    let mut merged = requested_obj.clone();

    // API identity is immutable. Copying it from the current resource also
    // prevents a request from changing a dynamic Kubernetes endpoint's GVK.
    for key in ["apiVersion", "kind"] {
        match current_obj.get(key) {
            Some(value) => {
                merged.insert(key.to_string(), value.clone());
            }
            None => {
                merged.remove(key);
            }
        }
    }

    // Preserve every server-created metadata field, while making the two
    // operator-owned maps exact replacements (absence clears the map).
    let mut merged_meta = current_meta.clone();
    for key in ["labels", "annotations"] {
        match requested_meta.get(key) {
            Some(value) => {
                merged_meta.insert(key.to_string(), value.clone());
            }
            None => {
                merged_meta.remove(key);
            }
        }
    }
    merged.insert("metadata".to_string(), Value::Object(merged_meta));

    match current_obj.get("status") {
        Some(status) => {
            merged.insert("status".to_string(), status.clone());
        }
        None => {
            merged.remove("status");
        }
    }

    Ok(Value::Object(merged))
}

/// YAML adapter for non-Kubernetes configuration centers.
///
/// The Admin API assigns a fresh `metadata.resourceVersion` before calling a
/// FileSystem or etcd writer. Unlike Kubernetes replace, where the current
/// resourceVersion is the optimistic-concurrency token, local backends must
/// persist that newly assigned request value so successful updates remain
/// monotonically versioned. Direct storage callers that omit it retain the
/// current value.
///
/// `metadata.generation` follows the same rule for the same reason: no local
/// backend advances it, so it is operator-supplied here rather than
/// server-owned as it is on the Kubernetes path. Dropping the requested value
/// would freeze generation at its create-time value forever, stalling every
/// `observedGeneration` reconciliation, and would diverge from the file-watcher
/// path, which takes generation straight from the operator's YAML.
pub(crate) fn merge_operator_update_yaml(current: &str, requested: &str) -> Result<String, ConfWriterError> {
    let current: Value = yaml_serde::from_str(current).map_err(|source| ConfWriterError::ParseError {
        context: "Failed to parse current resource for update",
        source: source.into(),
    })?;
    let requested: Value = yaml_serde::from_str(requested).map_err(|source| ConfWriterError::ParseError {
        context: "Failed to parse updated resource",
        source: source.into(),
    })?;
    let requested_meta = requested.get("metadata").and_then(Value::as_object);
    let operator_owned: Vec<(String, Value)> = ["resourceVersion", "generation"]
        .into_iter()
        .filter_map(|key| {
            requested_meta
                .and_then(|metadata| metadata.get(key))
                .map(|value| (key.to_string(), value.clone()))
        })
        .collect();
    let mut merged = merge_operator_update_values(current, requested)?;
    if !operator_owned.is_empty() {
        let metadata = merged
            .get_mut("metadata")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                ConfWriterError::ValidationError("merged resource metadata must be an object".to_string())
            })?;
        for (key, value) in operator_owned {
            metadata.insert(key, value);
        }
    }
    yaml_serde::to_string(&merged).map_err(|source| ConfWriterError::ParseError {
        context: "Failed to serialize merged resource update",
        source: source.into(),
    })
}

/// Enforce an optional client precondition against the resource read inside
/// the backend's atomic update window.
pub(crate) fn ensure_expected_resource_version(current: &str, expected: Option<&str>) -> Result<(), ConfWriterError> {
    let current: Value = yaml_serde::from_str(current).map_err(|source| ConfWriterError::ParseError {
        context: "Failed to parse current resourceVersion",
        source: source.into(),
    })?;
    ensure_expected_resource_version_value(&current, expected)
}

/// `expected == None` is the documented Admin API contract, not a legacy
/// carve-out: the write proceeds unconditionally and is last-writer-wins. The
/// Kubernetes backend calls this directly rather than through the `&str`
/// wrapper above, so between the two this is where every storage path that
/// evaluates the precondition locally arrives — the Kubernetes *delete* is the
/// one exception, handing it to the API Server as `DeleteParams.preconditions`.
/// See `skills/04-review/admin-api/intentional-design/admin-api-write-precondition-opt-in.md`.
pub(crate) fn ensure_expected_resource_version_value(
    current: &Value,
    expected: Option<&str>,
) -> Result<(), ConfWriterError> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let actual = current
        .get("metadata")
        .and_then(Value::as_object)
        .and_then(|metadata| metadata.get("resourceVersion"))
        .and_then(Value::as_str);
    if actual != Some(expected) {
        return Err(ConfWriterError::Conflict(format!(
            "resourceVersion precondition failed: expected {expected}, current {}",
            actual.unwrap_or("<missing>")
        )));
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../tests/unit/conf_mgr/conf_center/common_test/error_status_tests.rs"]
mod error_status_tests;

#[cfg(test)]
#[path = "../../../tests/unit/conf_mgr/conf_center/common_test/update_merge_tests.rs"]
mod update_merge_tests;

/// Tests for the FileSystem/etcd unknown-field diagnostic seam in
/// `parse_resource_content`. Unlike the
/// Kubernetes hand-written `Deserialize` impls, this path is NOT gated by
/// `strict_ingest_enabled` — see the comment at its call site above — so
/// these tests do not need to touch that process-global flag.
#[cfg(test)]
#[path = "../../../tests/unit/conf_mgr/conf_center/common_test/unknown_fields_seam_tests.rs"]
mod unknown_fields_seam_tests;

#[cfg(test)]
#[path = "../../../tests/support/conf_mgr/conf_center/common.rs"]
mod test_support;
#[cfg(test)]
pub(crate) use test_support::DEFAULT_CONTROLLER_NAME;
