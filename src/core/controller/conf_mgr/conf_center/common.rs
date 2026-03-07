//! Common configuration types shared across all configuration center backends
//!
//! This module contains types that are used by both FileSystem and Kubernetes modes.

use serde::{Deserialize, Serialize};

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
