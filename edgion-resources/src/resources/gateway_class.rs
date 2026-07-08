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
/// Most entries are Gateway API conformance feature names. `TCPRoute` and
/// `UDPRoute` are advertised here but are not part of any conformance profile,
/// so the Go side keeps them in an explicit allowlist instead of declaring them.
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
    "TCPRoute",
    "TLSRoute",
    "TLSRouteModeTerminate",
    "UDPRoute",
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
    "TLSRouteModeMixed",
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

#[cfg(test)]
mod feature_list_tests {
    use super::{SUPPORTED_FEATURES, UNSUPPORTED_OR_SKIPPED_FEATURES};
    use std::collections::BTreeSet;

    /// Relative path (from this crate's manifest dir) to the cross-language
    /// canonical list the Go conformance suite reads.
    const SHARED_LIST_PATH: &str = "../edgion-tests/conformance/supported_features.txt";

    fn assert_sorted_unique(list: &[&str], label: &str) {
        for pair in list.windows(2) {
            assert!(
                pair[0] < pair[1],
                "{label} must be sorted ascending with no duplicates, but {:?} is not strictly before {:?}",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn supported_features_sorted_and_unique() {
        assert_sorted_unique(SUPPORTED_FEATURES, "SUPPORTED_FEATURES");
    }

    #[test]
    fn unsupported_features_sorted_and_unique() {
        assert_sorted_unique(UNSUPPORTED_OR_SKIPPED_FEATURES, "UNSUPPORTED_OR_SKIPPED_FEATURES");
    }

    #[test]
    fn supported_and_unsupported_are_disjoint() {
        let supported: BTreeSet<&str> = SUPPORTED_FEATURES.iter().copied().collect();
        let unsupported: BTreeSet<&str> = UNSUPPORTED_OR_SKIPPED_FEATURES.iter().copied().collect();
        let overlap: Vec<&str> = supported.intersection(&unsupported).copied().collect();
        assert!(
            overlap.is_empty(),
            "a feature may be declared supported OR unsupported, never both: {overlap:?}"
        );
    }

    /// The Go conformance suite reads `supported_features.txt` to derive its
    /// declared feature set. This guards that the committed file is byte-equal
    /// to what the controller advertises, so the two never silently diverge.
    /// If this fails, regenerate the file from `SUPPORTED_FEATURES`.
    #[test]
    fn shared_list_file_matches_supported_features() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/",
            "../edgion-tests/conformance/supported_features.txt"
        );
        let raw = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("cannot read shared feature list at {SHARED_LIST_PATH}: {e}"));
        let from_file: Vec<&str> = raw
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .collect();
        assert_eq!(
            from_file, SUPPORTED_FEATURES,
            "{SHARED_LIST_PATH} is out of sync with SUPPORTED_FEATURES; regenerate it"
        );
    }
}
