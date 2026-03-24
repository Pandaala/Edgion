//! Kubernetes version detection and EndpointMode resolution
//!
//! Detects K8s API capabilities to determine whether to use
//! Endpoints or EndpointSlice resources, and performs preflight
//! checks for Gateway API CRD availability.

use crate::core::controller::conf_mgr::conf_center::EndpointMode;
use anyhow::Result;
use k8s_openapi::api::discovery::v1::EndpointSlice;
use kube::api::Api;
use kube::core::{ApiResource, DynamicObject, GroupVersionKind};
use kube::Client;
use std::collections::HashSet;

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

/// Gateway API resource definition for preflight CRD check.
struct GatewayApiResource {
    kind: &'static str,
    group: &'static str,
    version: &'static str,
}

/// All Gateway API resources that the controller watches.
///
/// This is the single source of truth for CRD preflight checks.
/// Keep in sync with the spawn calls in `KubernetesController::run_controllers()`.
const GATEWAY_API_RESOURCES: &[GatewayApiResource] = &[
    GatewayApiResource {
        kind: "GatewayClass",
        group: "gateway.networking.k8s.io",
        version: "v1",
    },
    GatewayApiResource {
        kind: "Gateway",
        group: "gateway.networking.k8s.io",
        version: "v1",
    },
    GatewayApiResource {
        kind: "HTTPRoute",
        group: "gateway.networking.k8s.io",
        version: "v1",
    },
    GatewayApiResource {
        kind: "GRPCRoute",
        group: "gateway.networking.k8s.io",
        version: "v1",
    },
    GatewayApiResource {
        kind: "TLSRoute",
        group: "gateway.networking.k8s.io",
        version: "v1",
    },
    GatewayApiResource {
        kind: "ReferenceGrant",
        group: "gateway.networking.k8s.io",
        version: "v1beta1",
    },
    GatewayApiResource {
        kind: "TCPRoute",
        group: "gateway.networking.k8s.io",
        version: "v1alpha2",
    },
    GatewayApiResource {
        kind: "UDPRoute",
        group: "gateway.networking.k8s.io",
        version: "v1alpha2",
    },
    GatewayApiResource {
        kind: "BackendTLSPolicy",
        group: "gateway.networking.k8s.io",
        version: "v1alpha3",
    },
];

/// All Gateway API kind names known to the preflight system.
pub fn gateway_api_kind_names() -> HashSet<&'static str> {
    GATEWAY_API_RESOURCES.iter().map(|r| r.kind).collect()
}

/// Validate that all entries in `no_watch_kinds` are recognized resource kinds.
/// Logs a warning for each unrecognized value.
pub fn validate_no_watch_kinds(no_watch_kinds: &[String]) {
    let known = gateway_api_kind_names();
    // Also accept Edgion and K8s-native kinds that appear in all_processor_kinds
    let other_known: HashSet<&str> = [
        "Secret",
        "Service",
        "Endpoints",
        "EndpointSlice",
        "EdgionTls",
        "EdgionPlugins",
        "EdgionStreamPlugins",
        "PluginMetaData",
        "LinkSys",
        "EdgionAcme",
        "EdgionGatewayConfig",
    ]
    .into_iter()
    .collect();

    for kind in no_watch_kinds {
        if !known.contains(kind.as_str()) && !other_known.contains(kind.as_str()) {
            tracing::warn!(
                component = "k8s_controller",
                kind = %kind,
                "Unrecognized kind in no_watch_kinds — typo? This value will have no effect."
            );
        }
    }
}

/// Preflight check: verify that all required Gateway API CRDs are installed.
///
/// For each Gateway API resource not in `no_watch_kinds`, attempts a list
/// with `limit=1`. A 404 means the CRD is missing. All missing CRDs are
/// collected and reported in a single error with install instructions.
pub async fn preflight_check_gateway_api_crds(client: &Client, no_watch_kinds: &HashSet<String>) -> Result<()> {
    tracing::info!(
        component = "k8s_controller",
        no_watch_kinds = ?no_watch_kinds,
        "Starting Gateway API CRD preflight check"
    );

    let mut missing = Vec::new();

    for res in GATEWAY_API_RESOURCES {
        if no_watch_kinds.contains(res.kind) {
            tracing::debug!(
                component = "k8s_controller",
                kind = res.kind,
                "Skipping CRD check (in no_watch_kinds)"
            );
            continue;
        }

        let gvk = GroupVersionKind::gvk(res.group, res.version, res.kind);
        let api_resource = ApiResource::from_gvk(&gvk);
        let api: Api<DynamicObject> = Api::all_with(client.clone(), &api_resource);

        match api.list(&kube::api::ListParams::default().limit(1)).await {
            Ok(_) => {
                tracing::debug!(
                    component = "k8s_controller",
                    kind = res.kind,
                    group = res.group,
                    version = res.version,
                    "CRD available"
                );
            }
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("404")
                    || err_str.contains("not found")
                    || err_str.contains("the server could not find the requested resource")
                {
                    tracing::warn!(
                        component = "k8s_controller",
                        kind = res.kind,
                        group = res.group,
                        version = res.version,
                        "CRD not found"
                    );
                    missing.push(res);
                } else {
                    // Non-404 errors (permission, network) — fail immediately with distinct message
                    return Err(anyhow::anyhow!(
                        "Failed to probe Gateway API CRD {} ({}/{}): {}. \
                         Check RBAC permissions and network connectivity.",
                        res.kind,
                        res.group,
                        res.version,
                        e
                    ));
                }
            }
        }
    }

    if missing.is_empty() {
        tracing::info!(component = "k8s_controller", "Gateway API CRD preflight check passed");
        return Ok(());
    }

    let missing_list: Vec<String> = missing
        .iter()
        .map(|r| format!("{} ({}/{})", r.kind, r.group, r.version))
        .collect();
    let skip_hint: Vec<&str> = missing.iter().map(|r| r.kind).collect();

    Err(anyhow::anyhow!(
        "Gateway API CRDs not found: {}\n\n\
         Install missing CRDs:\n  \
         kubectl apply -f https://github.com/kubernetes-sigs/gateway-api/releases/download/v1.2.1/experimental-install.yaml\n\n\
         Or skip these resources in config:\n  \
         conf_center.kubernetes.no_watch_kinds: {:?}",
        missing_list.join(", "),
        skip_hint,
    ))
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
