//! Kubernetes version detection for Gateway API CRDs.
//!
//! Performs preflight checks for Gateway API CRD availability and probes
//! API versions for ReferenceGrant.

use anyhow::Result;
use edgion_resources::gateway_api_support::{canonical_sources, GROUP};
use kube::core::GroupVersionKind;
use kube::error::DiscoveryError;
use kube::Client;
use std::collections::HashSet;

/// The API version detected for ReferenceGrant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceGrantVersion {
    V1,
    V1Beta1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatewayClassVersion {
    V1,
    V1Beta1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HTTPRouteVersion {
    V1,
    V1Beta1,
}

pub async fn probe_http_route_version(client: &Client) -> Result<Option<HTTPRouteVersion>> {
    for (version, selected) in [("v1", HTTPRouteVersion::V1), ("v1beta1", HTTPRouteVersion::V1Beta1)] {
        let gvk = GroupVersionKind::gvk(GROUP, version, "HTTPRoute");
        match is_kind_served(client, &gvk).await {
            Ok(true) => return Ok(Some(selected)),
            Ok(false) => continue,
            Err(error) => return Err(anyhow::anyhow!("Failed to probe HTTPRoute {version}: {error}")),
        }
    }
    Ok(None)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatewayVersion {
    V1,
    V1Beta1,
}

pub async fn probe_gateway_version(client: &Client) -> Result<Option<GatewayVersion>> {
    for (version, selected) in [("v1", GatewayVersion::V1), ("v1beta1", GatewayVersion::V1Beta1)] {
        let gvk = GroupVersionKind::gvk(GROUP, version, "Gateway");
        match is_kind_served(client, &gvk).await {
            Ok(true) => return Ok(Some(selected)),
            Ok(false) => continue,
            Err(error) => return Err(anyhow::anyhow!("Failed to probe Gateway {version}: {error}")),
        }
    }
    Ok(None)
}

pub async fn probe_gateway_class_version(client: &Client) -> Result<Option<GatewayClassVersion>> {
    for (version, selected) in [
        ("v1", GatewayClassVersion::V1),
        ("v1beta1", GatewayClassVersion::V1Beta1),
    ] {
        let gvk = GroupVersionKind::gvk(GROUP, version, "GatewayClass");
        match is_kind_served(client, &gvk).await {
            Ok(true) => return Ok(Some(selected)),
            Ok(false) => continue,
            Err(error) => return Err(anyhow::anyhow!("Failed to probe GatewayClass {version}: {error}")),
        }
    }
    Ok(None)
}

fn is_api_unavailable(error: &kube::Error) -> bool {
    matches!(error, kube::Error::Api(response) if response.code == 404)
        || matches!(
            error,
            kube::Error::Discovery(
                DiscoveryError::MissingKind(_)
                    | DiscoveryError::MissingApiGroup(_)
                    | DiscoveryError::MissingResource(_)
                    | DiscoveryError::EmptyApiGroup(_)
            )
        )
}

/// Check whether a GVK is served through Kubernetes API discovery.
///
/// Discovery avoids listing namespaced objects at cluster scope, so startup
/// remains compatible with controllers restricted to explicit namespaces.
async fn is_kind_served(client: &Client, gvk: &GroupVersionKind) -> Result<bool, kube::Error> {
    match kube::discovery::pinned_kind(client, gvk).await {
        Ok(_) => Ok(true),
        Err(error) if is_api_unavailable(&error) => Ok(false),
        Err(error) => Err(error),
    }
}

/// Detect which ReferenceGrant API version the cluster supports.
///
/// Tries v1 first (GA since Gateway API v1.1.0). Falls back to v1beta1 for
/// older clusters. Returns `None` if neither version is available.
pub async fn detect_reference_grant_version(client: &Client) -> Option<ReferenceGrantVersion> {
    match probe_reference_grant_version(client).await {
        Ok(version) => version,
        Err(error) => {
            tracing::warn!(
                component = "k8s_controller",
                error = %error,
                "Failed to detect ReferenceGrant API version"
            );
            None
        }
    }
}

/// Probe served ReferenceGrant versions without collapsing authorization or
/// transport failures into "CRD absent".
pub async fn probe_reference_grant_version(client: &Client) -> Result<Option<ReferenceGrantVersion>> {
    for (version, detected) in [
        ("v1", ReferenceGrantVersion::V1),
        ("v1beta1", ReferenceGrantVersion::V1Beta1),
    ] {
        let gvk = GroupVersionKind::gvk("gateway.networking.k8s.io", version, "ReferenceGrant");
        match is_kind_served(client, &gvk).await {
            Ok(true) => {
                tracing::info!(
                    component = "k8s_controller",
                    version = version,
                    "ReferenceGrant API detected"
                );
                return Ok(Some(detected));
            }
            Ok(false) => continue,
            Err(error) => {
                return Err(anyhow::anyhow!(
                    "Failed to probe ReferenceGrant API version {}: {}. Check RBAC permissions and network connectivity.",
                    version,
                    error
                ));
            }
        }
    }

    tracing::warn!(
        component = "k8s_controller",
        "Neither ReferenceGrant v1 nor v1beta1 available"
    );
    Ok(None)
}

/// All Gateway API kind names known to the preflight system.
pub fn gateway_api_kind_names() -> HashSet<&'static str> {
    canonical_sources().map(|r| r.kind).collect()
}

fn other_known_kind_names() -> HashSet<&'static str> {
    [
        "Secret",
        "Service",
        "EndpointSlice",
        "EdgionTls",
        "EdgionPlugins",
        "EdgionStreamPlugins",
        "EdgionConfigData",
        "LinkSys",
        "EdgionBackend",
        "EdgionAcme",
        "EdgionBackendTrafficPolicy",
        "EdgionGatewayConfig",
    ]
    .into_iter()
    .collect()
}

/// Validate that all entries in `no_watch_kinds` are recognized resource kinds.
/// Logs a warning for each unrecognized value.
pub fn validate_no_watch_kinds(no_watch_kinds: &[String]) {
    let known = gateway_api_kind_names();
    // Also accept Edgion and K8s-native kinds that appear in all_processor_kinds
    let other_known = other_known_kind_names();

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
/// For each Gateway API resource not in `no_watch_kinds`, queries API
/// discovery. An unavailable GVK means the CRD is missing. All missing CRDs are
/// collected and reported in a single error with install instructions.
pub async fn preflight_check_gateway_api_crds(
    client: &Client,
    no_watch_kinds: &HashSet<String>,
    reference_grant_version: Option<ReferenceGrantVersion>,
    gateway_class_version: Option<GatewayClassVersion>,
    gateway_version: Option<GatewayVersion>,
    http_route_version: Option<HTTPRouteVersion>,
) -> Result<()> {
    tracing::info!(
        component = "k8s_controller",
        no_watch_kinds = ?no_watch_kinds,
        "Starting Gateway API CRD preflight check"
    );

    let mut missing = Vec::new();

    for res in canonical_sources() {
        if no_watch_kinds.contains(res.kind) {
            tracing::debug!(
                component = "k8s_controller",
                kind = res.kind,
                "Skipping CRD check (in no_watch_kinds)"
            );
            continue;
        }

        if res.kind == "GatewayClass" {
            if gateway_class_version.is_none() {
                missing.push(res);
            }
            continue;
        }
        if res.kind == "Gateway" {
            if gateway_version.is_none() {
                missing.push(res);
            }
            continue;
        }
        if res.kind == "HTTPRoute" {
            if http_route_version.is_none() {
                missing.push(res);
            }
            continue;
        }
        if res.kind == "ReferenceGrant" {
            match reference_grant_version {
                Some(version) => {
                    tracing::debug!(
                        component = "k8s_controller",
                        kind = res.kind,
                        group = GROUP,
                        version = ?version,
                        "CRD available"
                    );
                }
                None => {
                    tracing::warn!(
                        component = "k8s_controller",
                        kind = res.kind,
                        group = GROUP,
                        versions = "v1,v1beta1",
                        "CRD not found"
                    );
                    missing.push(res);
                }
            }
            continue;
        }

        let gvk = GroupVersionKind::gvk(GROUP, res.version, res.kind);
        match is_kind_served(client, &gvk).await {
            Ok(true) => {
                tracing::debug!(
                    component = "k8s_controller",
                    kind = res.kind,
                    group = GROUP,
                    version = res.version,
                    "CRD available"
                );
            }
            Ok(false) => {
                tracing::warn!(
                    component = "k8s_controller",
                    kind = res.kind,
                    group = GROUP,
                    version = res.version,
                    "CRD not found"
                );
                missing.push(res);
            }
            Err(e) => {
                // Authorization and transport failures remain distinct from an absent CRD.
                return Err(anyhow::anyhow!(
                    "Failed to probe Gateway API CRD {} ({}/{}): {}. \
                     Check RBAC permissions and network connectivity.",
                    res.kind,
                    GROUP,
                    res.version,
                    e
                ));
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
            if matches!(r.kind, "ReferenceGrant" | "GatewayClass" | "Gateway" | "HTTPRoute") {
                format!("{} ({}/v1 or v1beta1)", r.kind, GROUP)
            } else {
                format!("{} ({}/{})", r.kind, GROUP, r.version)
            }
        })
        .collect();
    let skip_hint: Vec<&str> = missing.iter().map(|r| r.kind).collect();

    Err(anyhow::anyhow!(
        "Gateway API CRDs not found: {}\n\n\
         Install missing CRDs:\n  \
         kubectl apply -f https://github.com/kubernetes-sigs/gateway-api/releases/download/v1.6.2/standard-install.yaml\n  \
         # Or, if you need the experimental Gateway API resources:\n  \
         kubectl apply --server-side=true -f https://github.com/kubernetes-sigs/gateway-api/releases/download/v1.6.2/experimental-install.yaml\n\n\
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
    fn tls_route_preflight_matches_the_single_canonical_watch_and_status_type() {
        use edgion_resources::TLSRoute;
        use kube::Resource;
        let entries: Vec<_> = canonical_sources().filter(|entry| entry.kind == "TLSRoute").collect();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].version, TLSRoute::version(&()));
        assert_eq!(GROUP, TLSRoute::group(&()));
        assert_eq!(TLSRoute::api_version(&()), "gateway.networking.k8s.io/v1");
    }

    #[test]
    fn unavailable_discovery_results_are_classified_as_absent() {
        for error in [
            DiscoveryError::MissingKind("Gateway".into()),
            DiscoveryError::MissingApiGroup("gateway.networking.k8s.io".into()),
            DiscoveryError::MissingResource("gateways".into()),
            DiscoveryError::EmptyApiGroup("gateway.networking.k8s.io/v1".into()),
        ] {
            assert!(is_api_unavailable(&kube::Error::Discovery(error)));
        }
    }

    #[test]
    fn invalid_discovery_configuration_is_not_classified_as_absent() {
        let error = kube::Error::Discovery(DiscoveryError::InvalidGroupVersion("invalid".into()));
        assert!(!is_api_unavailable(&error));
    }

    #[test]
    fn only_api_not_found_is_classified_as_absent() {
        let response = |code| kube::error::ErrorResponse {
            status: "Failure".into(),
            message: "test".into(),
            reason: "test".into(),
            code,
        };
        assert!(is_api_unavailable(&kube::Error::Api(response(404))));
        assert!(!is_api_unavailable(&kube::Error::Api(response(403))));
    }

    #[test]
    fn backend_is_a_known_no_watch_kind() {
        assert!(other_known_kind_names().contains("EdgionBackend"));
    }
}
