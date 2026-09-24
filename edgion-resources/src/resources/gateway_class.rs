//! GatewayClass resource definition
//!
//! GatewayClass defines a class of Gateways that can be instantiated

use super::common::Condition;
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// GatewayClass defines a class of Gateways that can be instantiated
#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "gateway.networking.k8s.io",
    version = "v1",
    kind = "GatewayClass",
    plural = "gatewayclasses",
    status = "GatewayClassStatus"
)]
#[serde(rename_all = "camelCase")]
pub struct GatewayClassSpec {
    /// Controller-computed status for this deployment, transported to the Gateway.
    /// Never supplied by operators or used as the persisted status history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub current_status: Option<GatewayClassStatus>,

    /// ControllerName is the name of the controller
    pub controller_name: String,

    /// Description is a human-readable description of the class
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// ParametersRef references a resource that contains parameters
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters_ref: Option<ParametersReference>,
}

/// GatewayClassStatus describes the status of the GatewayClass resource.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct GatewayClassStatus {
    /// Conditions describe the current conditions of the GatewayClass.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<Condition>,

    /// SupportedFeatures is the set of features the GatewayClass supports.
    /// Each entry is an object with a `name` field (Gateway API v1.5+ format).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supported_features: Option<Vec<SupportedFeature>>,
}

/// A feature supported by a GatewayClass (Gateway API v1.5+ format).
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
pub struct SupportedFeature {
    /// The feature name (e.g., "Gateway", "HTTPRoute").
    pub name: String,
}

/// Canonical, single source of truth for the Gateway API features Edgion
/// advertises in `GatewayClass.status.supportedFeatures`.
///
/// Invariants (enforced by the unit tests below):
/// - sorted ascending by byte order (Gateway API requires ascending order),
/// - no duplicates,
/// - disjoint from [`UNSUPPORTED_OR_SKIPPED_FEATURES`],
/// - kept in sync with the committed `edgion-tests/conformance/supported_features.txt`,
///   which the Go conformance suite reads so the controller status and the
///   conformance declaration cannot silently drift.
///
/// Entries are Gateway API conformance feature names. A resource may still be
/// implemented without being advertised when its full profile has not passed.
pub const SUPPORTED_FEATURES: &[&str] = &[
    "BackendTLSPolicy",
    "BackendTLSPolicySANValidation",
    "GRPCRoute",
    "Gateway",
    "GatewayHTTPListenerIsolation",
    "GatewayHTTPSListenerDetectMisdirectedRequests",
    "HTTPRoute",
    "HTTPRoute303RedirectStatusCode",
    "HTTPRoute307RedirectStatusCode",
    "HTTPRoute308RedirectStatusCode",
    "HTTPRouteBackendProtocolH2C",
    "HTTPRouteBackendProtocolWebSocket",
    "HTTPRouteBackendRequestHeaderModification",
    "HTTPRouteBackendTimeout",
    "HTTPRouteCORS",
    "HTTPRouteDestinationPortMatching",
    "HTTPRouteHostRewrite",
    "HTTPRouteMethodMatching",
    "HTTPRouteNamedRouteRule",
    "HTTPRoutePathRedirect",
    "HTTPRoutePathRewrite",
    "HTTPRoutePortRedirect",
    "HTTPRouteQueryParamMatching",
    "HTTPRouteRequestMirror",
    "HTTPRouteRequestPercentageMirror",
    "HTTPRouteRequestTimeout",
    "HTTPRouteResponseHeaderModification",
    "HTTPRouteSchemeRedirect",
    "ReferenceGrant",
    "TLSRoute",
];

/// Gateway API features Edgion has evaluated and intentionally does NOT
/// advertise. The rationale for each lives in
/// `skills/08-kubernetes/01-gateway-api/01-not-implemented.md`.
///
/// This list is not serialized anywhere; it exists so the unit tests can guard
/// against a feature being added to [`SUPPORTED_FEATURES`] while still recorded
/// here as unsupported (the silent over-/under-declaration trap).
pub const UNSUPPORTED_OR_SKIPPED_FEATURES: &[&str] = &[
    // Status handling conforms, but data-plane enforcement (default / per-port
    // client-cert validation) does not pass conformance yet, so we do not
    // advertise it. See 01-not-implemented.md.
    "GatewayFrontendClientCertificateValidation",
    "GatewayInfrastructurePropagation",
    "GatewayPort8080",
    "GatewayStaticAddresses",
    "HTTPRouteParentRefPort",
    "HTTPRouteRequestMultipleMirrors",
    "ListenerSet",
    // v1 typed routing works on preconfigured listeners, but the v1.6.2
    // conformance profiles create new listener ports after process startup.
    "TCPRoute",
    "TLSRouteModeMixed",
    // A targeted TLS terminate case passes, but the full TLS profile still
    // fails this extended case after the other tests have run.
    "TLSRouteModeTerminate",
    "UDPRoute",
];

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ParametersReference {
    /// Group is the group of the referent
    pub group: String,

    /// Kind is the kind of the referent
    pub kind: String,

    /// Name is the name of the referent
    pub name: String,

    /// Namespace is the namespace of the referent
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
}

/// The Standard v1beta1 wire shape. Conversion retains metadata and status.
#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "gateway.networking.k8s.io",
    version = "v1beta1",
    kind = "GatewayClass",
    root = "GatewayClassBeta",
    plural = "gatewayclasses",
    status = "GatewayClassStatus"
)]
#[serde(rename_all = "camelCase")]
pub struct GatewayClassBetaSpec {
    pub controller_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters_ref: Option<ParametersReference>,
}

impl From<GatewayClassBeta> for GatewayClass {
    fn from(beta: GatewayClassBeta) -> Self {
        Self {
            metadata: beta.metadata,
            spec: GatewayClassSpec {
                current_status: None,
                controller_name: beta.spec.controller_name,
                description: beta.spec.description,
                parameters_ref: beta.spec.parameters_ref,
            },
            status: beta.status,
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/resources/gateway_class_test/feature_list_tests.rs"]
mod feature_list_tests;
