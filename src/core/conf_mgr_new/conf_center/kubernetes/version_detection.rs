//! Kubernetes version detection and EndpointMode resolution
//!
//! Detects K8s API capabilities to determine whether to use
//! Endpoints or EndpointSlice resources.

use crate::core::conf_mgr_new::conf_center::EndpointMode;
use anyhow::Result;
use k8s_openapi::api::discovery::v1::EndpointSlice;
use kube::api::Api;
use kube::Client;

/// Detect which endpoint mode to use based on K8s API capabilities.
///
/// Checks if EndpointSlice API (discovery.k8s.io/v1) is available.
/// EndpointSlice became stable in K8s 1.21.
///
/// Uses a simple approach: try to list EndpointSlices with limit=0.
/// If successful, the API is available; if 404 or error, fall back to Endpoints.
pub async fn detect_endpoint_mode(client: &Client) -> Result<EndpointMode> {
    tracing::info!(
        component = "k8s_controller",
        "Auto-detecting endpoint mode by querying K8s API"
    );

    // Try to access EndpointSlice API with limit=0 (returns empty list if available)
    let endpoint_slices: Api<EndpointSlice> = Api::all(client.clone());

    match endpoint_slices.list(&kube::api::ListParams::default().limit(1)).await {
        Ok(_) => {
            tracing::info!(
                component = "k8s_controller",
                api = "discovery.k8s.io/v1 EndpointSlice",
                "EndpointSlice API detected, using EndpointSlice mode"
            );
            Ok(EndpointMode::EndpointSlice)
        }
        Err(e) => {
            // Check if it's a "not found" error (API doesn't exist)
            let is_not_found = e.to_string().contains("404") || e.to_string().contains("not found");
            if is_not_found {
                tracing::info!(
                    component = "k8s_controller",
                    "EndpointSlice API not available, falling back to Endpoints mode"
                );
                Ok(EndpointMode::Endpoint)
            } else {
                // Other errors (permission, network, etc.) - log and default to EndpointSlice
                tracing::warn!(
                    component = "k8s_controller",
                    error = %e,
                    "Failed to detect EndpointSlice API, defaulting to EndpointSlice mode"
                );
                Ok(EndpointMode::EndpointSlice)
            }
        }
    }
}

/// Resolve final endpoint mode (handle Auto mode).
///
/// - Auto: Detect based on K8s API capabilities
/// - Endpoint/EndpointSlice: Use configured mode directly
pub async fn resolve_endpoint_mode(client: &Client, config_mode: EndpointMode) -> Result<EndpointMode> {
    match config_mode {
        EndpointMode::Auto => {
            tracing::info!(
                component = "k8s_controller",
                "Endpoint mode is 'auto', starting detection"
            );
            detect_endpoint_mode(client).await
        }
        mode => {
            tracing::info!(
                component = "k8s_controller",
                mode = ?mode,
                "Using manually configured endpoint mode"
            );
            Ok(mode)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_endpoint_mode_default() {
        assert_eq!(EndpointMode::default(), EndpointMode::Auto);
    }

    #[test]
    fn test_endpoint_mode_serialization() {
        let mode = EndpointMode::EndpointSlice;
        let yaml = serde_yaml::to_string(&mode).unwrap();
        assert_eq!(yaml.trim(), "endpoint_slice");

        let mode = EndpointMode::Auto;
        let yaml = serde_yaml::to_string(&mode).unwrap();
        assert_eq!(yaml.trim(), "auto");
    }
}
