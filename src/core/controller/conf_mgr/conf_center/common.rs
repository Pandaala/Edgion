//! Common configuration types shared across all configuration center backends
//!
//! This module contains types that are used by both FileSystem and Kubernetes modes.

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::types::resources::common::Condition;

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
pub fn phase1_kinds(endpoint_mode: EndpointMode) -> Vec<&'static str> {
    let mut kinds = vec![
        "GatewayClass",
        "Gateway",
        "Secret",
        "ConfigMap",
        "ReferenceGrant",
        "Service",
    ];
    if endpoint_mode.uses_endpoint() {
        kinds.push("Endpoints");
    }
    if endpoint_mode.uses_endpoint_slice() {
        kinds.push("EndpointSlice");
    }
    kinds
}

/// Endpoint discovery mode for Kubernetes
///
/// Controls which Kubernetes resource types are **synced/watched** for service endpoints.
/// This does NOT directly determine which resource is used for backend selection.
///
/// **Backend Selection Logic:**
/// - Auto/Both/EndpointSlice modes all default to using EndpointSlice for selection
/// - Only Endpoint mode uses Endpoints for selection
/// - Use `ServiceEndpoint` or `ServiceEndpointSlice` in BackendRef.kind to override
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EndpointMode {
    /// Sync only Endpoints resource (K8s 1.0+, legacy)
    ///
    /// The traditional Endpoints resource aggregates all endpoints into a single object.
    /// May have scalability issues with large numbers of endpoints (>1000).
    /// Backend selection will use Endpoints.
    Endpoint,

    /// Sync only EndpointSlice resource (K8s 1.21+, recommended)
    ///
    /// EndpointSlice is the modern replacement for Endpoints, providing better
    /// scalability by splitting endpoints into smaller chunks (default 100 per slice).
    /// Backend selection will use EndpointSlice.
    EndpointSlice,

    /// Sync both Endpoint and EndpointSlice resources
    ///
    /// Monitors both resource types simultaneously.
    /// Backend selection defaults to EndpointSlice.
    /// Use `ServiceEndpoint` in BackendRef.kind to force using Endpoints.
    ///
    /// Useful for:
    /// - Testing both modes in integration tests
    /// - Gradual migration from Endpoints to EndpointSlice
    Both,

    /// Auto-detect based on K8s API server version (default)
    ///
    /// - In Kubernetes mode: Queries the API server to determine the best option
    /// - In FileSystem mode: Defaults to EndpointSlice
    ///   Backend selection defaults to EndpointSlice.
    #[default]
    Auto,
}

impl EndpointMode {
    /// Check if this is the Endpoint mode
    pub fn is_endpoint(&self) -> bool {
        matches!(self, Self::Endpoint)
    }

    /// Check if this is the EndpointSlice mode
    pub fn is_endpoint_slice(&self) -> bool {
        matches!(self, Self::EndpointSlice)
    }

    /// Check if this is the Both mode
    pub fn is_both(&self) -> bool {
        matches!(self, Self::Both)
    }

    /// Check if this is the Auto mode
    pub fn is_auto(&self) -> bool {
        matches!(self, Self::Auto)
    }

    /// Check if EndpointSlice should be monitored
    ///
    /// Returns true for EndpointSlice and Both modes.
    pub fn uses_endpoint_slice(&self) -> bool {
        matches!(self, Self::EndpointSlice | Self::Both)
    }

    /// Check if Endpoints should be monitored
    ///
    /// Returns true for Endpoint and Both modes.
    pub fn uses_endpoint(&self) -> bool {
        matches!(self, Self::Endpoint | Self::Both)
    }
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

    /// Create a status for YAML parse error
    pub fn parse_error(error: &str) -> Self {
        Self::error("ParseError", error)
    }
}

// ============================================================================
// Shared resource parsing (used by both FileSystem and Etcd backends)
// ============================================================================

use crate::types::prelude_resources::{BackendTLSPolicy, BackendTLSPolicyAlpha3, TLSRoute, TLSRouteAlpha3};
use crate::types::{ResourceKind, ResourceMeta};
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
        return serde_yaml::from_str::<K>(content);
    }

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

fn extract_api_version(content: &str) -> Option<String> {
    let value: serde_yaml::Value = serde_yaml::from_str(content).ok()?;
    value.get("apiVersion")?.as_str().map(ToString::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_endpoint_mode_default() {
        let mode = EndpointMode::default();
        assert!(mode.is_auto());
    }

    #[test]
    fn test_endpoint_mode_serialize() {
        let mode = EndpointMode::EndpointSlice;
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, "\"endpoint_slice\"");
    }

    #[test]
    fn test_endpoint_mode_deserialize() {
        let mode: EndpointMode = serde_json::from_str("\"endpoint\"").unwrap();
        assert!(mode.is_endpoint());
    }

    #[test]
    fn test_endpoint_mode_uses_endpoint_slice() {
        // EndpointSlice and Both should sync EndpointSlice
        assert!(EndpointMode::EndpointSlice.uses_endpoint_slice());
        assert!(EndpointMode::Both.uses_endpoint_slice());
        // Endpoint and Auto should not
        assert!(!EndpointMode::Endpoint.uses_endpoint_slice());
        assert!(!EndpointMode::Auto.uses_endpoint_slice());
    }

    #[test]
    fn test_endpoint_mode_uses_endpoint() {
        // Endpoint and Both should sync Endpoints
        assert!(EndpointMode::Endpoint.uses_endpoint());
        assert!(EndpointMode::Both.uses_endpoint());
        // EndpointSlice and Auto should not
        assert!(!EndpointMode::EndpointSlice.uses_endpoint());
        assert!(!EndpointMode::Auto.uses_endpoint());
    }

    #[test]
    fn test_endpoint_mode_both_syncs_all() {
        let mode = EndpointMode::Both;
        assert!(mode.uses_endpoint());
        assert!(mode.uses_endpoint_slice());
        assert!(mode.is_both());
    }
}
