//! Common configuration types shared across all configuration center backends
//!
//! This module contains types that are used by both FileSystem and Kubernetes modes.

use chrono::Utc;
use serde::{Deserialize, Serialize};

use edgion_resources::resources::common::Condition;

// ============================================================================
// Shared controller constants and helpers (FileSystem and Etcd backends)
// ============================================================================

/// Default controller name used by FileSystem and Etcd backends.
pub const DEFAULT_CONTROLLER_NAME: &str = "edgion.io/gateway-controller";

/// Returns the Phase-1 foundation resource kinds for FileSystem/Etcd backends.
///
/// These must be ready before Phase-2 resources start processing, because
/// Phase-2 resources look up GatewayClass, Gateway, Secret, Service, etc.
/// during their first `handle()` call.
///
/// The K8s backend has its own variant that additionally filters `no_watch_kinds`.
pub fn phase1_kinds() -> Vec<&'static str> {
    vec![
        "GatewayClass",
        "Gateway",
        "Secret",
        "ConfigMap",
        "ReferenceGrant",
        "Service",
        "EndpointSlice",
    ]
}

// ============================================================================
// Shared status types (used by both FileSystem and Etcd backends)
// ============================================================================

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
                type_: "Ready".to_string(),
                status: "False".to_string(),
                reason: reason.to_string(),
                message: message.to_string(),
                last_transition_time: Utc::now().to_rfc3339(),
                observed_generation: None,
            }],
        }
    }
}

// ============================================================================
// Shared resource parsing (used by both FileSystem and Etcd backends)
// ============================================================================

use edgion_resources::prelude_resources::{BackendTLSPolicy, BackendTLSPolicyAlpha3, TLSRoute, TLSRouteAlpha3};
use edgion_resources::{ResourceKind, ResourceMeta};
use kube::Resource;
use serde::de::DeserializeOwned;
use std::fmt::Debug;

/// Parse YAML content into a resource, with legacy API version support.
///
/// Handles transparent conversion of `v1alpha3` resources (BackendTLSPolicy, TLSRoute)
/// to their canonical `v1` types.
pub fn parse_resource_content<K>(content: &str) -> serde_yaml::Result<K>
where
    K: ResourceMeta + Resource + Clone + Send + Sync + Debug + serde::Serialize + DeserializeOwned + 'static,
{
    let kind = K::resource_kind();

    if kind != ResourceKind::BackendTLSPolicy && kind != ResourceKind::TLSRoute {
        serde_yaml::from_str::<K>(content)
    } else {
        match extract_api_version(content).as_deref() {
            Some("gateway.networking.k8s.io/v1alpha3") if kind == ResourceKind::BackendTLSPolicy => {
                let legacy: BackendTLSPolicyAlpha3 = serde_yaml::from_str(content)?;
                let canonical: BackendTLSPolicy = legacy.into();
                let normalized = serde_yaml::to_string(&canonical)?;
                serde_yaml::from_str::<K>(&normalized)
            }
            Some("gateway.networking.k8s.io/v1alpha3") if kind == ResourceKind::TLSRoute => {
                let legacy: TLSRouteAlpha3 = serde_yaml::from_str(content)?;
                let canonical: TLSRoute = legacy.into();
                let normalized = serde_yaml::to_string(&canonical)?;
                serde_yaml::from_str::<K>(&normalized)
            }
            _ => serde_yaml::from_str::<K>(content),
        }
    }
}

fn extract_api_version(content: &str) -> Option<String> {
    let value: serde_yaml::Value = serde_yaml::from_str(content).ok()?;
    value.get("apiVersion")?.as_str().map(ToString::to_string)
}
