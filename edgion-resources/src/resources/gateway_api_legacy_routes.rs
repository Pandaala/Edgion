//! Legacy Gateway API wire identities converted before canonical processing.

use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    BackendTLSPolicy, BackendTLSPolicySpec, BackendTLSPolicyStatus, GRPCRoute, GRPCRouteSpec, GRPCRouteStatus,
    TCPRoute, TCPRouteSpec, TCPRouteStatus, TLSRoute, TLSRouteSpec, TLSRouteStatus, UDPRoute, UDPRouteSpec,
    UDPRouteStatus,
};

#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "gateway.networking.k8s.io",
    version = "v1alpha2",
    kind = "GRPCRoute",
    root = "GRPCRouteAlpha2",
    plural = "grpcroutes",
    namespaced,
    status = "GRPCRouteStatus"
)]
pub struct GRPCRouteAlpha2Spec {
    #[serde(flatten)]
    pub canonical: GRPCRouteSpec,
}

#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "gateway.networking.k8s.io",
    version = "v1alpha3",
    kind = "TLSRoute",
    root = "TLSRouteAlpha3",
    plural = "tlsroutes",
    namespaced,
    status = "TLSRouteStatus"
)]
pub struct TLSRouteAlpha3Spec {
    #[serde(flatten)]
    pub canonical: TLSRouteSpec,
}

#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "gateway.networking.k8s.io",
    version = "v1alpha3",
    kind = "BackendTLSPolicy",
    root = "BackendTLSPolicyAlpha3",
    plural = "backendtlspolicies",
    namespaced,
    status = "BackendTLSPolicyStatus"
)]
pub struct BackendTLSPolicyAlpha3Spec {
    #[serde(flatten)]
    pub canonical: BackendTLSPolicySpec,
}

#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "gateway.networking.k8s.io",
    version = "v1alpha2",
    kind = "TCPRoute",
    root = "TCPRouteAlpha2",
    plural = "tcproutes",
    namespaced,
    status = "TCPRouteStatus"
)]
pub struct TCPRouteAlpha2Spec {
    #[serde(flatten)]
    pub canonical: TCPRouteSpec,
}

#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "gateway.networking.k8s.io",
    version = "v1alpha2",
    kind = "UDPRoute",
    root = "UDPRouteAlpha2",
    plural = "udproutes",
    namespaced,
    status = "UDPRouteStatus"
)]
pub struct UDPRouteAlpha2Spec {
    #[serde(flatten)]
    pub canonical: UDPRouteSpec,
}

macro_rules! canonical_from_legacy {
    ($source:ty, $target:ty) => {
        impl From<$source> for $target {
            fn from(source: $source) -> Self {
                Self {
                    metadata: source.metadata,
                    spec: source.spec.canonical,
                    status: source.status,
                }
            }
        }
    };
}

canonical_from_legacy!(GRPCRouteAlpha2, GRPCRoute);
canonical_from_legacy!(TLSRouteAlpha3, TLSRoute);
canonical_from_legacy!(BackendTLSPolicyAlpha3, BackendTLSPolicy);
canonical_from_legacy!(TCPRouteAlpha2, TCPRoute);
canonical_from_legacy!(UDPRouteAlpha2, UDPRoute);
