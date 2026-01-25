//! Common configuration types shared across all configuration center backends
//!
//! This module contains types that are used by both FileSystem and Kubernetes modes.

use serde::{Deserialize, Serialize};

/// Endpoint discovery mode for Kubernetes
///
/// Determines which Kubernetes resource type to use for service endpoint discovery.
/// This setting affects how the gateway discovers backend pod addresses.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EndpointMode {
    /// Use Endpoints resource (K8s 1.0+, legacy)
    ///
    /// The traditional Endpoints resource aggregates all endpoints into a single object.
    /// May have scalability issues with large numbers of endpoints (>1000).
    Endpoint,

    /// Use EndpointSlice resource (K8s 1.21+, recommended)
    ///
    /// EndpointSlice is the modern replacement for Endpoints, providing better
    /// scalability by splitting endpoints into smaller chunks (default 100 per slice).
    EndpointSlice,

    /// Auto-detect based on K8s API server version (default)
    ///
    /// - In Kubernetes mode: Queries the API server to determine the best option
    /// - In FileSystem mode: Defaults to EndpointSlice
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

    /// Check if this is the Auto mode
    pub fn is_auto(&self) -> bool {
        matches!(self, Self::Auto)
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
}
