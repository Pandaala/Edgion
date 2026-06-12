//! Kubernetes version detection for Gateway API CRDs.
//!
//! Performs preflight checks for Gateway API CRD availability and probes
//! API versions for ReferenceGrant, TLSRoute, and BackendTLSPolicy.

use anyhow::Result;
use kube::api::Api;
use kube::core::{ApiResource, DynamicObject, GroupVersionKind};
use kube::Client;
use std::collections::HashSet;

/// The API version detected for ReferenceGrant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceGrantVersion {
    V1,
    V1Beta1,
}

/// The API version detected for BackendTLSPolicy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendTLSPolicyVersion {
    V1,
    V1Alpha3,
}

/// Detect which ReferenceGrant API version the cluster supports.
///
/// Tries v1 first (GA since Gateway API v1.1.0). Falls back to v1beta1 for
/// older clusters. Returns `None` if neither version is available.
pub async fn detect_reference_grant_version(client: &Client) -> Option<ReferenceGrantVersion> {
    // Try v1 first
    let gvk_v1 = GroupVersionKind::gvk("gateway.networking.k8s.io", "v1", "ReferenceGrant");
    let api_resource_v1 = ApiResource::from_gvk(&gvk_v1);
    let api_v1: Api<DynamicObject> = Api::all_with(client.clone(), &api_resource_v1);

    if api_v1.list(&kube::api::ListParams::default().limit(1)).await.is_ok() {
        tracing::info!(component = "k8s_controller", "ReferenceGrant v1 API detected");
        return Some(ReferenceGrantVersion::V1);
    }

    // Fall back to v1beta1
    let gvk_beta = GroupVersionKind::gvk("gateway.networking.k8s.io", "v1beta1", "ReferenceGrant");
    let api_resource_beta = ApiResource::from_gvk(&gvk_beta);
    let api_beta: Api<DynamicObject> = Api::all_with(client.clone(), &api_resource_beta);

    if api_beta.list(&kube::api::ListParams::default().limit(1)).await.is_ok() {
        tracing::info!(
            component = "k8s_controller",
            "ReferenceGrant v1beta1 API detected (v1 not available)"
        );
        return Some(ReferenceGrantVersion::V1Beta1);
    }

    tracing::warn!(
        component = "k8s_controller",
        "Neither ReferenceGrant v1 nor v1beta1 available"
    );
    None
}

/// Detect which BackendTLSPolicy API version the cluster supports.
///
/// Tries v1 first (GA in newer Gateway API releases). Falls back to v1alpha3
/// for clusters that still expose the legacy served version.
pub async fn detect_backend_tls_policy_version(client: &Client) -> Option<BackendTLSPolicyVersion> {
    match probe_backend_tls_policy_version(client).await {
        Ok(version) => version,
        Err(err) => {
            tracing::warn!(
                component = "k8s_controller",
                error = %err,
                "Failed to detect BackendTLSPolicy API version"
            );
            None
        }
    }
}

/// Probe served BackendTLSPolicy versions.
///
/// Returns `Ok(Some(...))` when any supported version is available, `Ok(None)`
/// when the CRD is absent, and `Err(...)` for non-404 failures such as RBAC or
/// connectivity problems.
pub async fn probe_backend_tls_policy_version(client: &Client) -> Result<Option<BackendTLSPolicyVersion>> {
    for (version, detected) in [
        ("v1", BackendTLSPolicyVersion::V1),
        ("v1alpha3", BackendTLSPolicyVersion::V1Alpha3),
    ] {
        let gvk = GroupVersionKind::gvk("gateway.networking.k8s.io", version, "BackendTLSPolicy");
        let api_resource = ApiResource::from_gvk(&gvk);
        let api: Api<DynamicObject> = Api::all_with(client.clone(), &api_resource);

        match api.list(&kube::api::ListParams::default().limit(1)).await {
            Ok(_) => {
                tracing::info!(
                    component = "k8s_controller",
                    version = version,
                    "BackendTLSPolicy API detected"
                );
                return Ok(Some(detected));
            }
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("404")
                    || err_str.contains("not found")
                    || err_str.contains("the server could not find the requested resource")
                {
                    continue;
                }

                return Err(anyhow::anyhow!(
                    "Failed to probe BackendTLSPolicy API version {}: {}. \
                     Check RBAC permissions and network connectivity.",
                    version,
                    e
                ));
            }
        }
    }

    tracing::warn!(
        component = "k8s_controller",
        "Neither BackendTLSPolicy v1 nor v1alpha3 available"
    );
    Ok(None)
}

/// The API version detected for TLSRoute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TLSRouteVersion {
    V1,
    V1Alpha3,
}

/// Detect which TLSRoute API version the cluster supports.
///
/// Tries v1 first (Standard in Gateway API v1.5.0). Falls back to v1alpha3
/// for clusters running Gateway API v1.4.0 experimental.
pub async fn detect_tls_route_version(client: &Client) -> Option<TLSRouteVersion> {
    match probe_tls_route_version(client).await {
        Ok(version) => version,
        Err(err) => {
            tracing::warn!(
                component = "k8s_controller",
                error = %err,
                "Failed to detect TLSRoute API version"
            );
            None
        }
    }
}

/// Probe served TLSRoute versions.
///
/// Returns `Ok(Some(...))` when any supported version is available, `Ok(None)`
/// when the CRD is absent, and `Err(...)` for non-404 failures such as RBAC or
/// connectivity problems.
pub async fn probe_tls_route_version(client: &Client) -> Result<Option<TLSRouteVersion>> {
    for (version, detected) in [("v1", TLSRouteVersion::V1), ("v1alpha3", TLSRouteVersion::V1Alpha3)] {
        let gvk = GroupVersionKind::gvk("gateway.networking.k8s.io", version, "TLSRoute");
        let api_resource = ApiResource::from_gvk(&gvk);
        let api: Api<DynamicObject> = Api::all_with(client.clone(), &api_resource);

        match api.list(&kube::api::ListParams::default().limit(1)).await {
            Ok(_) => {
                tracing::info!(component = "k8s_controller", version = version, "TLSRoute API detected");
                return Ok(Some(detected));
            }
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("404")
                    || err_str.contains("not found")
                    || err_str.contains("the server could not find the requested resource")
                {
                    continue;
                }

                return Err(anyhow::anyhow!(
                    "Failed to probe TLSRoute API version {}: {}. \
                     Check RBAC permissions and network connectivity.",
                    version,
                    e
                ));
            }
        }
    }

    tracing::warn!(
        component = "k8s_controller",
        "Neither TLSRoute v1 nor v1alpha3 available"
    );
    Ok(None)
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
        version: "v1", // preflight overrides via multi-version probe
    },
    GatewayApiResource {
        kind: "ReferenceGrant",
        group: "gateway.networking.k8s.io",
        version: "v1",
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
        version: "v1",
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

        if res.kind == "BackendTLSPolicy" {
            match probe_backend_tls_policy_version(client).await? {
                Some(BackendTLSPolicyVersion::V1) => {
                    tracing::debug!(
                        component = "k8s_controller",
                        kind = res.kind,
                        group = res.group,
                        version = "v1",
                        "CRD available"
                    );
                }
                Some(BackendTLSPolicyVersion::V1Alpha3) => {
                    tracing::debug!(
                        component = "k8s_controller",
                        kind = res.kind,
                        group = res.group,
                        version = "v1alpha3",
                        "CRD available"
                    );
                }
                None => {
                    tracing::warn!(
                        component = "k8s_controller",
                        kind = res.kind,
                        group = res.group,
                        versions = "v1,v1alpha3",
                        "CRD not found"
                    );
                    missing.push(res);
                }
            }
            continue;
        }

        if res.kind == "TLSRoute" {
            match probe_tls_route_version(client).await? {
                Some(TLSRouteVersion::V1) => {
                    tracing::debug!(
                        component = "k8s_controller",
                        kind = res.kind,
                        group = res.group,
                        version = "v1",
                        "CRD available"
                    );
                }
                Some(TLSRouteVersion::V1Alpha3) => {
                    tracing::debug!(
                        component = "k8s_controller",
                        kind = res.kind,
                        group = res.group,
                        version = "v1alpha3",
                        "CRD available"
                    );
                }
                None => {
                    tracing::warn!(
                        component = "k8s_controller",
                        kind = res.kind,
                        group = res.group,
                        versions = "v1,v1alpha3",
                        "CRD not found"
                    );
                    missing.push(res);
                }
            }
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
        .map(|r| {
            if r.kind == "BackendTLSPolicy" || r.kind == "TLSRoute" {
                format!("{} ({}/v1 or v1alpha3)", r.kind, r.group)
            } else {
                format!("{} ({}/{})", r.kind, r.group, r.version)
            }
        })
        .collect();
    let skip_hint: Vec<&str> = missing.iter().map(|r| r.kind).collect();

    Err(anyhow::anyhow!(
        "Gateway API CRDs not found: {}\n\n\
         Install missing CRDs:\n  \
         kubectl apply -f https://github.com/kubernetes-sigs/gateway-api/releases/download/v1.5.0/standard-install.yaml\n  \
         # Or, if you need legacy served versions such as BackendTLSPolicy v1alpha3:\n  \
         kubectl apply --server-side=true -f https://github.com/kubernetes-sigs/gateway-api/releases/download/v1.5.0/experimental-install.yaml\n\n\
         Or skip these resources in config:\n  \
         conf_center.kubernetes.no_watch_kinds: {:?}",
        missing_list.join(", "),
        skip_hint,
    ))
}
