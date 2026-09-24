//! Common configuration types shared across all configuration center backends
//!
//! This module contains types that are used by both FileSystem and Kubernetes modes.

use edgion_common::time::now_rfc3339;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

use crate::conf_mgr::conf_center::traits::ConfWriterError;

use edgion_resources::resources::common::Condition;

// ============================================================================
// Shared controller constants and helpers (FileSystem and Etcd backends)
// ============================================================================

/// Explicit fixture identity; runtime identities always come from configuration.
#[cfg(test)]
pub const DEFAULT_CONTROLLER_NAME: &str = "edgion.io/gateway-controller";

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
/// Canonical sources are admitted directly. ReferenceGrant v1beta1 has an
/// explicit decoder and converter at every non-Kubernetes ingress path.
pub fn validate_gateway_api_source(kind: ResourceKind, value: &serde_yaml::Value) -> Result<(), String> {
    use edgion_resources::gateway_api_support::{canonical_sources, GROUP};

    let mut sources = canonical_sources().filter(|source| source.kind == kind.as_str());
    let Some(source) = sources.next() else {
        return Ok(());
    };
    if sources.next().is_some() {
        return Err(format!("{} has ambiguous canonical source versions", kind.as_str()));
    }

    let actual_kind = value.get("kind").and_then(serde_yaml::Value::as_str);
    if actual_kind != Some(source.kind) {
        return Err(format!("{} kind must be {}", kind.as_str(), source.kind));
    }
    let selected_version = if matches!(
        kind,
        ResourceKind::ReferenceGrant | ResourceKind::GatewayClass | ResourceKind::Gateway | ResourceKind::HTTPRoute
    ) && value.get("apiVersion").and_then(serde_yaml::Value::as_str)
        == Some("gateway.networking.k8s.io/v1beta1")
    {
        "v1beta1"
    } else {
        source.version
    };
    let expected_api_version = format!("{GROUP}/{selected_version}");
    if value.get("apiVersion").and_then(serde_yaml::Value::as_str) != Some(expected_api_version.as_str()) {
        return Err(format!("{} apiVersion must be {expected_api_version}", kind.as_str()));
    }
    Ok(())
}

/// Kubernetes CRUD still calls these typed version guards before writing.
pub fn validate_backend_tls_policy_api_version(api_version: Option<&str>) -> Result<(), String> {
    validate_canonical_v1("BackendTLSPolicy", api_version)
}

pub fn validate_tls_route_api_version(api_version: Option<&str>) -> Result<(), String> {
    validate_canonical_v1("TLSRoute", api_version)
}

fn validate_canonical_v1(kind: &str, api_version: Option<&str>) -> Result<(), String> {
    const VERSION: &str = "gateway.networking.k8s.io/v1";
    if api_version == Some(VERSION) {
        Ok(())
    } else {
        Err(format!("{kind} apiVersion must be {VERSION}"))
    }
}

/// Parse YAML content into a resource.
///
/// Gateway API sources are checked against the canonical inventory first.
pub fn parse_resource_content<K>(content: &str) -> serde_yaml::Result<K>
where
    K: ResourceMeta + Resource + Clone + Send + Sync + Debug + serde::Serialize + DeserializeOwned + 'static,
{
    let kind = K::resource_kind();

    validate_resource_api_version::<K>(content)?;

    let beta_source = serde_yaml::from_str::<serde_yaml::Value>(content)?
        .get("apiVersion")
        .and_then(serde_yaml::Value::as_str)
        == Some("gateway.networking.k8s.io/v1beta1");
    let mut parsed = if kind == ResourceKind::Gateway && beta_source {
        let beta: edgion_resources::GatewayBeta = serde_yaml::from_str(content)?;
        let canonical: edgion_resources::Gateway = beta.into();
        serde_yaml::from_value(serde_yaml::to_value(canonical)?)?
    } else if kind == ResourceKind::HTTPRoute && beta_source {
        let beta: edgion_resources::HTTPRouteBeta = serde_yaml::from_str(content)?;
        let canonical: edgion_resources::HTTPRoute = beta.into();
        serde_yaml::from_value(serde_yaml::to_value(canonical)?)?
    } else if kind == ResourceKind::GatewayClass && beta_source {
        let beta: edgion_resources::GatewayClassBeta = serde_yaml::from_str(content)?;
        let canonical: edgion_resources::GatewayClass = beta.into();
        serde_yaml::from_value(serde_yaml::to_value(canonical)?)?
    } else if kind == ResourceKind::ReferenceGrant && beta_source {
        let beta: edgion_resources::ReferenceGrantBeta = serde_yaml::from_str(content)?;
        let canonical: edgion_resources::ReferenceGrant = beta.into();
        serde_yaml::from_value(serde_yaml::to_value(canonical)?)?
    } else {
        serde_yaml::from_str::<K>(content)?
    };

    // FileSystem/etcd unknown-field diagnostic: the Kubernetes watcher
    // gets this via the hand-written `Deserialize` impls on `EdgionPluginsSpec`,
    // `EdgionStreamPluginsSpec`, `EdgionConfigDataSpec`, and `LinkSysSpec`, gated by
    // `strict_ingest_enabled`. That gate means
    // "Kubernetes ingress" specifically (see its doc comment), so it is NOT
    // consulted here — FileSystem/etcd content is checked unconditionally.
    if matches!(
        kind,
        ResourceKind::EdgionPlugins
            | ResourceKind::EdgionStreamPlugins
            | ResourceKind::EdgionConfigData
            | ResourceKind::LinkSys
    ) {
        populate_unknown_fields_non_fatal(kind, content, &mut parsed);
    }

    Ok(parsed)
}

/// Validate API-version contracts that must be enforced before typed
/// deserialization can discard the submitted TypeMeta.
///
/// This is shared by FileSystem/etcd ingestion and the Admin create, update,
/// and validate endpoints so every ingress applies the same atomic API
/// promotion boundary.
pub fn validate_resource_api_version<K>(content: &str) -> serde_yaml::Result<()>
where
    K: ResourceMeta,
{
    if edgion_resources::gateway_api_support::canonical_sources()
        .any(|source| source.kind == K::resource_kind().as_str())
    {
        let value: serde_yaml::Value = serde_yaml::from_str(content)?;
        validate_gateway_api_source(K::resource_kind(), &value).map_err(serde_yaml::Error::custom)?;
    }

    Ok(())
}

#[cfg(test)]
mod gateway_api_version_guard_tests {
    use super::validate_resource_api_version;
    use edgion_resources::{
        BackendTLSPolicy, Gateway, GatewayClass, HTTPRoute, ReferenceGrant, TCPRoute, TLSRoute, UDPRoute,
    };

    #[test]
    fn gateway_class_beta_preserves_source_fields_and_status() {
        let raw = serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1beta1", "kind": "GatewayClass",
            "metadata": {"name": "edge", "uid": "uid-1", "resourceVersion": "42", "generation": 3},
            "spec": {"controllerName": "example.com/edge", "description": "edge class",
                "parametersRef": {"group": "edgion.io", "kind": "EdgionGatewayConfig", "name": "config", "namespace": "infra"}},
            "status": {"conditions": [{"type": "Accepted", "status": "True", "reason": "Accepted",
                "message": "ready", "lastTransitionTime": "2026-01-01T00:00:00Z"}],
                "supportedFeatures": [{"name": "Gateway"}]}
        });
        let parsed: GatewayClass = super::parse_resource_content(&raw.to_string()).unwrap();
        let canonical = serde_json::to_value(parsed).unwrap();
        assert_eq!(canonical["apiVersion"], "gateway.networking.k8s.io/v1");
        for key in ["metadata", "status"] {
            assert_eq!(canonical[key], raw[key]);
        }
        assert_eq!(canonical["spec"], raw["spec"]);
        for (api_version, kind) in [
            ("gateway.networking.k8s.io/v1alpha2", "GatewayClass"),
            ("gateway.networking.k8s.io/v1beta1", "Other"),
        ] {
            let mut invalid = raw.clone();
            invalid["apiVersion"] = serde_json::json!(api_version);
            invalid["kind"] = serde_json::json!(kind);
            assert!(super::parse_resource_content::<GatewayClass>(&invalid.to_string()).is_err());
        }
        let mut future = raw;
        future["spec"]["futureStandardField"] = serde_json::json!({"mode": "future"});
        let parsed: GatewayClass = super::parse_resource_content(&future.to_string()).unwrap();
        assert_eq!(parsed.spec.controller_name, "example.com/edge");
        assert_eq!(parsed.metadata.resource_version.as_deref(), Some("42"));
    }

    #[test]
    fn gateway_beta_source_converts_and_preserves_known_unsupported_fields() {
        let source = serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1beta1", "kind": "Gateway",
            "metadata": {"name": "edge", "namespace": "demo", "resourceVersion": "17"},
            "spec": {"gatewayClassName": "edgion",
                "listeners": [{"name": "https", "port": 443, "protocol": "HTTPS"}],
                "infrastructure": {"labels": {"team": "web"}}}
        });
        let gateway: Gateway = super::parse_resource_content(&source.to_string()).unwrap();
        assert_eq!(gateway.metadata.resource_version.as_deref(), Some("17"));
        assert_eq!(gateway.spec.listeners.as_ref().unwrap()[0].name, "https");
        assert!(gateway.spec.infrastructure.is_some());
        let mut future = source;
        future["spec"].as_object_mut().unwrap().remove("infrastructure");
        future["spec"]["futureStandardField"] = serde_json::json!(true);
        assert!(super::parse_resource_content::<Gateway>(&future.to_string()).is_ok());
    }

    #[test]
    fn reference_grant_sources_convert_to_the_same_canonical_rules() {
        let document = |version: &str| {
            format!(
            "apiVersion: gateway.networking.k8s.io/{version}\nkind: ReferenceGrant\nmetadata:\n  name: allow\n  namespace: backend\n  resourceVersion: \"17\"\n  labels: {{team: edge}}\nspec:\n  from:\n    - {{group: gateway.networking.k8s.io, kind: HTTPRoute, namespace: app}}\n    - {{group: gateway.networking.k8s.io, kind: GRPCRoute, namespace: grpc}}\n  to:\n    - {{group: '', kind: Service, name: exact}}\n    - {{group: '', kind: Secret}}\n"
        )
        };
        let v1: ReferenceGrant = super::parse_resource_content(&document("v1")).unwrap();
        let beta: ReferenceGrant = super::parse_resource_content(&document("v1beta1")).unwrap();
        assert_eq!(beta.metadata, v1.metadata);
        assert_eq!(beta.spec.from, v1.spec.from);
        assert_eq!(beta.spec.to, v1.spec.to);
        assert_eq!(beta.metadata.resource_version.as_deref(), Some("17"));
        assert_eq!(beta.spec.from.len(), 2);
        assert_eq!(beta.spec.to.len(), 2);
    }

    #[test]
    fn reference_grant_rejects_invalid_source_identity_and_required_rules() {
        let valid = "apiVersion: gateway.networking.k8s.io/v1beta1\nkind: ReferenceGrant\nmetadata:\n  name: allow\n  namespace: backend\nspec:\n  from: []\n  to: []\n";
        for bad in [
            valid.replace("kind: ReferenceGrant", "kind: ReferenceGrantBeta"),
            valid.replace("gateway.networking.k8s.io", "other.example"),
            valid.replace("v1beta1", "v1alpha2"),
            valid.replace("  from: []\n", ""),
            valid.replace("  to: []\n", ""),
        ] {
            assert!(
                super::parse_resource_content::<ReferenceGrant>(&bad).is_err(),
                "accepted {bad}"
            );
        }
    }

    #[test]
    fn canonical_raw_guards_reject_missing_and_old_versions() {
        for kind in ["TLSRoute", "BackendTLSPolicy"] {
            let document = |version: Option<&str>| {
                let api_version = version
                    .map(|value| format!("apiVersion: {value}\n"))
                    .unwrap_or_default();
                format!("{api_version}kind: {kind}\nmetadata:\n  name: test\nspec: {{}}\n")
            };
            for version in [None, Some("gateway.networking.k8s.io/v1beta1")] {
                let raw = document(version);
                let result = if kind == "TLSRoute" {
                    validate_resource_api_version::<TLSRoute>(&raw)
                } else {
                    validate_resource_api_version::<BackendTLSPolicy>(&raw)
                };
                assert!(result.is_err(), "{kind} accepted {version:?}");
            }
            let raw = document(Some("gateway.networking.k8s.io/v1"));
            let result = if kind == "TLSRoute" {
                validate_resource_api_version::<TLSRoute>(&raw)
            } else {
                validate_resource_api_version::<BackendTLSPolicy>(&raw)
            };
            assert!(result.is_ok(), "{kind} rejected v1");
        }
    }

    #[test]
    fn canonical_sources_reject_bad_type_meta_in_yaml_and_json() {
        type Check = fn(&str) -> serde_yaml::Result<()>;
        for (kind, check) in [
            ("GatewayClass", validate_resource_api_version::<GatewayClass> as Check),
            ("Gateway", validate_resource_api_version::<Gateway> as Check),
            ("HTTPRoute", validate_resource_api_version::<HTTPRoute> as Check),
            ("TCPRoute", validate_resource_api_version::<TCPRoute> as Check),
            ("UDPRoute", validate_resource_api_version::<UDPRoute> as Check),
            (
                "ReferenceGrant",
                validate_resource_api_version::<ReferenceGrant> as Check,
            ),
        ] {
            for api_version in ["gateway.networking.k8s.io/v1beta1", "other.example/v1"] {
                if matches!(kind, "ReferenceGrant" | "GatewayClass" | "Gateway") && api_version.ends_with("/v1beta1") {
                    continue;
                }
                let yaml = format!("apiVersion: {api_version}\nkind: {kind}\nmetadata:\n  name: test\nspec: {{}}\n");
                let json =
                    serde_json::json!({"apiVersion":api_version,"kind":kind,"metadata":{"name":"test"},"spec":{}});
                assert!(check(&yaml).is_err(), "{kind} accepted {api_version} YAML");
                assert!(check(&json.to_string()).is_err(), "{kind} accepted {api_version} JSON");
            }
            let missing = format!("kind: {kind}\nmetadata:\n  name: test\nspec: {{}}\n");
            let malformed = format!("apiVersion: [v1]\nkind: {kind}\nmetadata:\n  name: test\nspec: {{}}\n");
            let wrong_kind =
                "apiVersion: gateway.networking.k8s.io/v1\nkind: Other\nmetadata:\n  name: test\nspec: {}\n";
            assert!(check(&missing).is_err());
            assert!(check(&malformed).is_err());
            assert!(check(wrong_kind).is_err());
        }
    }

    #[test]
    fn tcp_and_udp_v1_ingress_reject_alpha_then_accept_repaired_document() {
        for (kind, check) in [
            (
                "TCPRoute",
                validate_resource_api_version::<TCPRoute> as fn(&str) -> serde_yaml::Result<()>,
            ),
            (
                "UDPRoute",
                validate_resource_api_version::<UDPRoute> as fn(&str) -> serde_yaml::Result<()>,
            ),
        ] {
            let alpha = format!(
                "apiVersion: gateway.networking.k8s.io/v1alpha2\nkind: {kind}\nmetadata:\n  name: route\n  namespace: default\nspec:\n  parentRefs:\n    - name: edge\n  rules:\n    - backendRefs:\n        - name: backend\n          port: 8080\n"
            );
            assert!(check(&alpha).is_err(), "{kind} accepted alpha source");
            let canonical = alpha.replace("/v1alpha2", "/v1");
            assert!(check(&canonical).is_ok(), "{kind} rejected v1 source");
            if kind == "TCPRoute" {
                let route: TCPRoute = super::parse_resource_content(&canonical).unwrap();
                assert_eq!(
                    route.spec.rules.as_ref().unwrap()[0].backend_refs.as_ref().unwrap()[0].name,
                    "backend"
                );
            } else {
                let route: UDPRoute = super::parse_resource_content(&canonical).unwrap();
                assert_eq!(
                    route.spec.rules.as_ref().unwrap()[0].backend_refs.as_ref().unwrap()[0].name,
                    "backend"
                );
            }
        }
    }
}

/// Populate `spec.unknown_fields` on an already-parsed resource supported by
/// the unknown-field diagnostic, for the FileSystem/etcd loader seam.
///
/// Non-fatal by design: unlike the Kubernetes hook, this
/// runs on YAML content that may contain shapes `serde_json::Value` cannot
/// represent (a non-string map key, `!!binary`, a YAML tag). This diagnostic
/// must never turn an otherwise-loadable document into a load failure, so any
/// parse/convert error here just skips the check and the caller's typed
/// parse (already completed above) is returned unchanged.
///
/// `K` is generic over every resource kind `parse_resource_content` serves,
/// so `Any::downcast_mut` is the minimal way to reach the concrete
/// supported spec fields without adding a trait method to every resource type
/// for a diagnostic only four kinds need. The
/// `K: 'static` bound `parse_resource_content` already carries makes this
/// sound.
fn populate_unknown_fields_non_fatal<K: 'static>(kind: ResourceKind, content: &str, parsed: &mut K) {
    let yaml_value: serde_yaml::Value = match serde_yaml::from_str(content) {
        Ok(v) => v,
        Err(err) => {
            tracing::debug!(
                component = "conf_center",
                event = "unknown_fields_yaml_parse_skip",
                kind = ?kind,
                error = %err,
                "skipping unknown-field check: content did not re-parse as YAML"
            );
            return;
        }
    };
    let json_value: Value = match serde_json::to_value(&yaml_value) {
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
    let current: Value = serde_yaml::from_str(current).map_err(|source| ConfWriterError::ParseError {
        context: "Failed to parse current resource for update",
        source: source.into(),
    })?;
    let requested: Value = serde_yaml::from_str(requested).map_err(|source| ConfWriterError::ParseError {
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
    serde_yaml::to_string(&merged).map_err(|source| ConfWriterError::ParseError {
        context: "Failed to serialize merged resource update",
        source: source.into(),
    })
}

/// Enforce an optional client precondition against the resource read inside
/// the backend's atomic update window.
pub(crate) fn ensure_expected_resource_version(current: &str, expected: Option<&str>) -> Result<(), ConfWriterError> {
    let current: Value = serde_yaml::from_str(current).map_err(|source| ConfWriterError::ParseError {
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
mod error_status_tests {
    use super::*;

    fn stored(status: &str, reason: &str, message: &str, ltt: &str) -> String {
        format!(
            r#"{{"conditions":[{{"type":"Ready","status":"{status}","reason":"{reason}","message":"{message}","lastTransitionTime":"{ltt}"}}]}}"#
        )
    }

    #[test]
    fn identical_error_skips_write() {
        let existing = stored("False", "ParseError", "bad yaml", "2026-01-01T00:00:00Z");
        assert!(
            ErrorStatus::error_for_persist("ParseError", "bad yaml", Some(&existing)).is_none(),
            "re-scanning the same broken resource must not rewrite its status"
        );
    }

    #[test]
    fn changed_message_writes_but_preserves_ltt() {
        let existing = stored("False", "ParseError", "bad yaml at line 1", "2026-01-01T00:00:00Z");
        let status = ErrorStatus::error_for_persist("ParseError", "bad yaml at line 9", Some(&existing))
            .expect("a different message must be persisted");
        assert_eq!(
            status.conditions[0].last_transition_time, "2026-01-01T00:00:00Z",
            "status did not flip, so lastTransitionTime must be carried over"
        );
    }

    #[test]
    fn no_prior_status_writes_fresh() {
        let status = ErrorStatus::error_for_persist("ParseError", "bad yaml", None).expect("first error must persist");
        assert_eq!(status.conditions[0].status, "False");
        assert!(!status.conditions[0].last_transition_time.is_empty());
    }

    #[test]
    fn written_error_status_is_recognized_as_a_sidecar() {
        let persisted = serde_json::to_string(&ErrorStatus::error("ParseError", "bad yaml")).unwrap();
        assert!(
            is_error_status_sidecar_json(&persisted),
            "what write_error_status persists must be detectable as a sidecar"
        );
        let mismatch = serde_json::to_string(&ErrorStatus::error("IdentityMismatch", "key drifted")).unwrap();
        assert!(is_error_status_sidecar_json(&mismatch));
    }

    #[test]
    fn handler_owned_status_is_never_a_sidecar() {
        // A route status carries no condition of the sidecar type, so the
        // post-repair cleanup must never delete it.
        let route_status = serde_json::json!({
            "parents": [{
                "controllerName": "edgion.io/gateway-controller",
                "conditions": [{"type": "Accepted", "status": "True", "reason": "Accepted"}]
            }]
        });
        assert!(!is_error_status_sidecar(&route_status));
        assert!(!is_error_status_sidecar(&serde_json::json!({})));
        assert!(!is_error_status_sidecar(&serde_json::Value::Null));
    }

    #[test]
    fn unparsable_status_is_not_a_sidecar() {
        assert!(!is_error_status_sidecar_json("not json"));
    }
}

#[cfg(test)]
mod update_merge_tests {
    use super::*;

    #[test]
    fn operator_update_preserves_server_fields_without_restoring_current_spec() {
        let current = serde_json::json!({
            "apiVersion": "gateway.networking.k8s.io/v1",
            "kind": "HTTPRoute",
            "metadata": {
                "name": "route",
                "namespace": "default",
                "uid": "uid-1",
                "resourceVersion": "42",
                "generation": 7,
                "managedFields": [{"manager": "controller"}],
                "creationTimestamp": "2026-07-15T00:00:00Z",
                "deletionTimestamp": "2026-07-16T00:00:00Z",
                "ownerReferences": [{"name": "owner"}],
                "finalizers": ["edgion.io/finalizer"],
                "labels": {"old": "label"},
                "annotations": {"old": "annotation"}
            },
            "spec": {
                "hostnames": ["old.example"],
                "resolvedListeners": [{"name": "runtime-only"}]
            },
            "status": {"conditions": [{"type": "Accepted", "status": "True"}]}
        });
        let requested = serde_json::json!({
            "apiVersion": "spoofed/v9",
            "kind": "Gateway",
            "metadata": {
                "name": "renamed",
                "namespace": "other",
                "uid": "spoofed",
                "resourceVersion": "999",
                "finalizers": [],
                "labels": {"new": "label"}
            },
            "spec": {"hostnames": ["new.example"]},
            "status": {"conditions": []},
            "data": {"operator": "field"}
        });

        let merged = merge_operator_update_values(current, requested).expect("merge must succeed");

        assert_eq!(merged["apiVersion"], "gateway.networking.k8s.io/v1");
        assert_eq!(merged["kind"], "HTTPRoute");
        assert_eq!(merged["metadata"]["name"], "route");
        assert_eq!(merged["metadata"]["namespace"], "default");
        assert_eq!(merged["metadata"]["uid"], "uid-1");
        assert_eq!(merged["metadata"]["resourceVersion"], "42");
        assert_eq!(merged["metadata"]["generation"], 7);
        assert_eq!(merged["metadata"]["managedFields"][0]["manager"], "controller");
        assert_eq!(merged["metadata"]["creationTimestamp"], "2026-07-15T00:00:00Z");
        assert_eq!(merged["metadata"]["deletionTimestamp"], "2026-07-16T00:00:00Z");
        assert_eq!(merged["metadata"]["ownerReferences"][0]["name"], "owner");
        assert_eq!(merged["metadata"]["finalizers"][0], "edgion.io/finalizer");
        assert_eq!(merged["metadata"]["labels"]["new"], "label");
        assert!(merged["metadata"].get("annotations").is_none());
        assert_eq!(merged["spec"]["hostnames"][0], "new.example");
        assert!(merged["spec"].get("resolvedListeners").is_none());
        assert_eq!(merged["status"]["conditions"][0]["type"], "Accepted");
        assert_eq!(merged["data"]["operator"], "field");
    }

    #[test]
    fn non_kubernetes_yaml_update_uses_generated_request_resource_version() {
        let current = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: route
  namespace: default
  resourceVersion: "42"
  uid: uid-1
spec:
  hostnames: [old.example]
"#;
        let requested = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: route
  namespace: default
  resourceVersion: "43"
spec:
  hostnames: [new.example]
"#;

        let merged = merge_operator_update_yaml(current, requested).expect("non-Kubernetes merge must succeed");
        let merged: Value = serde_yaml::from_str(&merged).expect("merged resource must remain valid YAML");
        assert_eq!(merged["metadata"]["resourceVersion"], "43");
        assert_eq!(merged["metadata"]["uid"], "uid-1");
        assert_eq!(merged["spec"]["hostnames"][0], "new.example");
    }

    #[test]
    fn non_kubernetes_yaml_update_uses_requested_generation() {
        let current = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: gw
  namespace: default
  generation: 1
  uid: uid-1
spec:
  listeners: [{name: http}]
"#;
        let requested = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: gw
  namespace: default
  generation: 11
spec:
  listeners: [{name: http-v10}]
"#;

        let merged = merge_operator_update_yaml(current, requested).expect("non-Kubernetes merge must succeed");
        let merged: Value = serde_yaml::from_str(&merged).expect("merged resource must remain valid YAML");
        assert_eq!(merged["metadata"]["generation"], 11);
        assert_eq!(merged["metadata"]["uid"], "uid-1");
        assert_eq!(merged["spec"]["listeners"][0]["name"], "http-v10");
    }

    #[test]
    fn non_kubernetes_yaml_update_retains_generation_when_request_omits_it() {
        let current = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: gw
  namespace: default
  generation: 7
spec:
  listeners: [{name: http}]
"#;
        let requested = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: gw
  namespace: default
spec:
  listeners: [{name: http-v1}]
"#;

        let merged = merge_operator_update_yaml(current, requested).expect("non-Kubernetes merge must succeed");
        let merged: Value = serde_yaml::from_str(&merged).expect("merged resource must remain valid YAML");
        assert_eq!(merged["metadata"]["generation"], 7);
    }
}

/// Tests for the FileSystem/etcd unknown-field diagnostic seam in
/// `parse_resource_content`. Unlike the
/// Kubernetes hand-written `Deserialize` impls, this path is NOT gated by
/// `strict_ingest_enabled` — see the comment at its call site above — so
/// these tests do not need to touch that process-global flag.
#[cfg(test)]
mod unknown_fields_seam_tests {
    use super::parse_resource_content;
    use edgion_resources::prelude_resources::{
        EdgionConfigData, EdgionPlugins, EdgionStreamPlugins, HTTPRoute, LinkSys,
    };

    const TYPO_PLUGINS: &str = r#"
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: typo-plugins
  namespace: default
spec:
  requestPlugins:
    - type: IpRestriction
      config:
        defaultAction: allow
        denys: []
"#;

    const CLEAN_PLUGINS: &str = r#"
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: clean-plugins
  namespace: default
spec:
  requestPlugins:
    - type: IpRestriction
      config:
        defaultAction: allow
        deny: []
"#;

    const TYPO_STREAM_PLUGINS: &str = r#"
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: typo-stream-plugins
  namespace: default
spec:
  plugins:
    - type: IpRestriction
      config:
        defaultAction: allow
        denys: []
"#;

    const TYPO_CONFIG_DATA: &str = r#"
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: typo-config-data
  namespace: default
spec:
  data:
    type: IpList
    config:
      items:
        - name: office
          cidrs: []
          cidrz: [10.0.0.0/8]
"#;

    const TYPO_PROXY_PROTOCOL_CONFIG_DATA: &str = r#"
apiVersion: edgion.io/v1
kind: EdgionConfigData
metadata:
  name: proxy-policy
  namespace: default
spec:
  data:
    type: ProxyProtocolTrust
    config:
      mode: required
      typo: true
"#;

    const TYPO_LINK_SYS: &str = r#"
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: typo-redis
  namespace: default
spec:
  type: redis
  config:
    endpoints: [redis://127.0.0.1:6379]
    timeout:
      connect: 3s
      conect: 20
"#;

    #[test]
    fn edgion_plugins_typo_populates_unknown_fields_with_exact_path() {
        let parsed: EdgionPlugins = parse_resource_content(TYPO_PLUGINS).expect("well-formed aside from the typo");
        assert_eq!(
            parsed.spec.unknown_fields,
            vec!["requestPlugins[0].config.denys".to_string()]
        );
    }

    #[test]
    fn edgion_plugins_clean_document_yields_no_findings() {
        let parsed: EdgionPlugins = parse_resource_content(CLEAN_PLUGINS).expect("clean document must parse");
        assert!(parsed.spec.unknown_fields.is_empty());
    }

    #[test]
    fn edgion_stream_plugins_typo_populates_unknown_fields_with_exact_path() {
        let parsed: EdgionStreamPlugins =
            parse_resource_content(TYPO_STREAM_PLUGINS).expect("well-formed aside from the typo");
        assert_eq!(parsed.spec.unknown_fields, vec!["plugins[0].config.denys".to_string()]);
    }

    #[test]
    fn edgion_stream_plugins_clean_document_yields_no_findings() {
        let clean = TYPO_STREAM_PLUGINS.replace("denys", "deny");
        let parsed: EdgionStreamPlugins = parse_resource_content(&clean).expect("clean document must parse");
        assert!(parsed.spec.unknown_fields.is_empty());
    }

    #[test]
    fn edgion_config_data_typo_populates_unknown_fields_with_exact_path() {
        let parsed: EdgionConfigData =
            parse_resource_content(TYPO_CONFIG_DATA).expect("well-formed aside from the typo");
        assert_eq!(
            parsed.spec.unknown_fields,
            vec!["data.config.items[0].cidrz".to_string()]
        );
    }

    #[test]
    fn proxy_protocol_config_data_typo_parses_and_is_fatal() {
        let parsed: EdgionConfigData = parse_resource_content(TYPO_PROXY_PROTOCOL_CONFIG_DATA)
            .expect("ProxyProtocolTrust typo must remain loadable for status reporting");
        assert_eq!(parsed.spec.unknown_fields, ["data.config.typo"]);
        assert_eq!(
            parsed.validate_pre_parse(),
            ["ProxyProtocolTrust invalid: policy validation failed"]
        );
    }

    #[test]
    fn edgion_config_data_misc_payload_remains_open() {
        let misc = TYPO_CONFIG_DATA.replace(
            "type: IpList\n    config:\n      items:\n        - name: office\n          cidrs: []\n          cidrz: [10.0.0.0/8]",
            "type: Misc\n    config:\n      vendor:\n        arbitrary: [1, 2, 3]",
        );
        let parsed: EdgionConfigData = parse_resource_content(&misc).expect("Misc must accept arbitrary JSON");
        assert!(parsed.spec.unknown_fields.is_empty());
    }

    #[test]
    fn link_sys_typo_populates_unknown_fields_with_exact_path() {
        let parsed: LinkSys = parse_resource_content(TYPO_LINK_SYS).expect("well-formed aside from the typo");
        assert_eq!(parsed.spec.unknown_fields, ["config.timeout.conect"]);
    }

    #[test]
    fn link_sys_clean_and_removed_webhook_fields_report_findings() {
        let clean = TYPO_LINK_SYS.replace("      conect: 20\n", "");
        let parsed: LinkSys = parse_resource_content(&clean).expect("clean LinkSys must parse");
        assert!(parsed.spec.unknown_fields.is_empty());

        let webhook = r#"
apiVersion: edgion.io/v1
kind: LinkSys
metadata: {name: legacy-webhook, namespace: default}
spec:
  type: webhook
  config:
    target: {kind: Service, name: resolver, port: 8080}
    auth: {vendor: {arbitrary: true}}
    defaultHeaders: {X-Legacy: {arbitrary: true}}
    requestMethod: {arbitrary: true}
"#;
        let parsed: LinkSys = parse_resource_content(webhook).expect("removed fields must not block parsing");
        assert_eq!(
            parsed.spec.unknown_fields,
            ["config.auth", "config.defaultHeaders", "config.requestMethod"]
        );
    }

    /// The non-fatal guarantee: a document that re-parses as a `serde_yaml::Value`
    /// but cannot convert to `serde_json::Value` (a non-string mapping key, which
    /// JSON has no representation for) must still load successfully via the normal
    /// typed parse — the diagnostic just skips itself instead of failing the load.
    /// This guarantee is tested directly against the seam helper
    /// rather than through `parse_resource_content`'s full round trip: a
    /// document with a mapping key that is itself a YAML sequence (`? [a, b]`)
    /// parses fine into `serde_yaml::Value` (YAML permits non-scalar keys),
    /// but JSON has no representation for a non-string object key, so
    /// `serde_json::to_value` fails on it — the exact failure shape this
    /// non-fatal seam guards against.
    ///
    /// This cannot be exercised through `parse_resource_content`'s normal
    /// entry point: a struct field's key deserializer requires a string
    /// identifier regardless of whether the field is recognized, so any
    /// document with a non-scalar top-level key fails the typed parse itself
    /// (`serde_yaml::from_str::<K>`) before this seam ever runs — i.e. the
    /// class of document that breaks the JSON round-trip is already rejected
    /// upstream, and the seam's non-fatal skip is defense-in-depth for that
    /// gap rather than something reachable end-to-end today. Calling the
    /// helper directly with an already-parsed clean object isolates exactly
    /// the required behavior: the check skips itself and leaves the
    /// object's `unknown_fields` untouched.
    #[test]
    fn json_conversion_failure_is_skipped_and_leaves_object_untouched() {
        let mut parsed: EdgionPlugins = parse_resource_content(CLEAN_PLUGINS).expect("clean document must parse");
        let unconvertible_content = "? [a, b]\n: nested-key-seq\n";

        super::populate_unknown_fields_non_fatal(
            edgion_resources::ResourceKind::EdgionPlugins,
            unconvertible_content,
            &mut parsed,
        );

        assert!(
            parsed.spec.unknown_fields.is_empty(),
            "a YAML->JSON conversion failure must be skipped, not surfaced as spurious findings"
        );
    }

    /// An unsupported kind goes through `parse_resource_content` completely
    /// unchanged — no findings machinery is invoked for it.
    #[test]
    fn unsupported_kind_is_unaffected() {
        let content = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: r1
  namespace: default
spec:
  hostnames: [example.com]
"#;
        let parsed: HTTPRoute = parse_resource_content(content).expect("HTTPRoute must still parse normally");
        assert_eq!(parsed.metadata.name.as_deref(), Some("r1"));
    }
}
