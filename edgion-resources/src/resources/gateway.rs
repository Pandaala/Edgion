//! Gateway resource definition
//!
//! Gateway defines a network gateway

use k8s_openapi::{api::core::v1::Secret, ByteString};

crate::redact::impl_redacted_debug!(
    GatewayBackendTLS,
    ResolvedFrontendCAReference,
    ResolvedServingCertificateReference
);
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::common::Condition;
use super::proxy_protocol::ResolvedInboundProxyProtocol;

/// Gateway API listener cardinality bound. Kubernetes admission normally
/// enforces this; FS/etcd and forged conf-sync objects must enforce it too.
pub const MAX_GATEWAY_LISTENERS: usize = 64;
pub const MAX_GATEWAY_FRONTEND_PER_PORT: usize = 64;

/// Gateway defines a network gateway
#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "gateway.networking.k8s.io",
    version = "v1",
    kind = "Gateway",
    plural = "gateways",
    status = "GatewayStatus",
    namespaced
)]
#[serde(rename_all = "camelCase")]
pub struct GatewaySpec {
    /// Controller-computed status for this deployment, transported to the Gateway.
    /// Never supplied by operators or used as the persisted status history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub current_status: Option<GatewayStatus>,

    /// GatewayClassName used for this Gateway
    pub gateway_class_name: String,

    /// Listeners associated with this Gateway
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(max = MAX_GATEWAY_LISTENERS))]
    pub listeners: Option<Vec<Listener>>,

    /// Addresses requested for this Gateway
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub addresses: Option<Vec<GatewayAddress>>,

    /// Gateway-level TLS configuration (Gateway API v1.5)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<GatewaySpecTLS>,

    /// Known upstream fields unsupported by this controller. Kept through
    /// source conversion for per-object status; omitted from conf_sync.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub allowed_listeners: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub infrastructure: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub default_scope: Option<serde_json::Value>,

    /// Controller-resolved inbound PROXY protocol policy transported to the
    /// Gateway. This is not an operator-authored Gateway API field.
    #[serde(default)]
    #[schemars(skip)]
    pub resolved_inbound_proxy_protocol: ResolvedInboundProxyProtocol,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub resolved_attachment_proof: Option<super::edgion_tls::GatewayAttachmentProof>,
}

/// Standard v1beta1 Gateway source. Shared nested types preserve the complete
/// listener, TLS, route-attachment, address, and status representation.
#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    group = "gateway.networking.k8s.io",
    version = "v1beta1",
    kind = "Gateway",
    root = "GatewayBeta",
    plural = "gateways",
    status = "GatewayStatus",
    namespaced
)]
#[serde(rename_all = "camelCase")]
pub struct GatewayBetaSpec {
    pub gateway_class_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listeners: Option<Vec<Listener>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub addresses: Option<Vec<GatewayAddress>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<GatewaySpecTLS>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_listeners: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub infrastructure: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_scope: Option<serde_json::Value>,
}

impl From<GatewayBeta> for Gateway {
    fn from(beta: GatewayBeta) -> Self {
        Self {
            metadata: beta.metadata,
            spec: GatewaySpec {
                current_status: None,
                gateway_class_name: beta.spec.gateway_class_name,
                listeners: beta.spec.listeners,
                addresses: beta.spec.addresses,
                tls: beta.spec.tls,
                allowed_listeners: beta.spec.allowed_listeners,
                infrastructure: beta.spec.infrastructure,
                default_scope: beta.spec.default_scope,
                resolved_inbound_proxy_protocol: ResolvedInboundProxyProtocol::default(),
                resolved_attachment_proof: None,
            },
            status: beta.status,
        }
    }
}

pub fn gateway_attachment_proof(gateway: &Gateway) -> super::edgion_tls::GatewayAttachmentProof {
    gateway
        .spec
        .resolved_attachment_proof
        .clone()
        .unwrap_or_else(|| super::edgion_tls::GatewayAttachmentProof {
            instance: "unresolved".to_string(),
            generation: -1,
            epoch: "unresolved".to_string(),
            attachment_digest: "unresolved".to_string(),
        })
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Listener {
    /// Name of the Listener
    pub name: String,

    /// Hostname associated with the Listener
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        length(max = crate::gwapi_types::HOSTNAME_MAX_LEN),
        pattern(crate::gwapi_types::HOSTNAME_PATTERN)
    )]
    pub hostname: Option<String>,

    /// Port on which the Listener is listening
    #[schemars(range(min = crate::gwapi_types::PORT_MIN, max = crate::gwapi_types::PORT_MAX))]
    pub port: i32,

    /// Protocol of the Listener
    pub protocol: String,

    /// TLS configuration for the Listener
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<GatewayTLSConfig>,

    /// AllowedRoutes defines which Routes may be attached
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_routes: Option<AllowedRoutes>,
}

/// Validation mode for frontend client certificate verification.
///
/// Mirrors Gateway API v1.5 `FrontendTLSValidation.mode`. Stored as a typed
/// enum so typos produce a deserialization error instead of silently falling
/// back to the strict default.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq, Default)]
pub enum FrontendValidationMode {
    /// Reject connections whose client certificate fails validation (default).
    #[default]
    AllowValidOnly,
    /// Request a client certificate but accept the handshake even when the
    /// certificate is missing or invalid. Validation outcome is exposed to
    /// downstream layers; intended for migration/monitoring scenarios.
    AllowInsecureFallback,
}

/// FrontendTLSValidation holds configuration for validating client certificates
/// at the Gateway listener level (Gateway API standard mTLS).
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FrontendTLSValidation {
    /// CACertificateRefs references Secrets or ConfigMaps containing CA certificate bundles
    /// for validating client certificates. Must contain `ca.crt` key.
    #[serde(deserialize_with = "deserialize_frontend_ca_refs")]
    #[schemars(length(min = 1, max = 16))]
    pub ca_certificate_refs: Vec<FrontendCACertificateRef>,

    /// Validation mode (default: AllowValidOnly).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<FrontendValidationMode>,
}

/// Gateway API v1.5 frontend trust-anchor reference.
///
/// Unlike `SecretObjectReference`, the standard API requires `group`, `kind`,
/// and `name` to be present. Keeping a dedicated wire type prevents the
/// controller's non-Kubernetes config centers from accepting shapes that the
/// upstream Gateway CRD would reject.
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FrontendCACertificateRef {
    #[schemars(
        length(max = 253),
        pattern(r"^$|^[a-z0-9]([-a-z0-9]*[a-z0-9])?(\.[a-z0-9]([-a-z0-9]*[a-z0-9])?)*$")
    )]
    pub group: String,
    #[schemars(length(min = 1, max = 63), pattern(r"^[a-zA-Z]([-a-zA-Z0-9]*[a-zA-Z0-9])?$"))]
    pub kind: String,
    #[schemars(length(min = 1, max = 253))]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 63), pattern(r"^[a-z0-9]([-a-z0-9]*[a-z0-9])?$"))]
    pub namespace: Option<String>,
}

fn deserialize_frontend_ca_refs<'de, D>(deserializer: D) -> Result<Vec<FrontendCACertificateRef>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let refs = Vec::<FrontendCACertificateRef>::deserialize(deserializer)?;
    if refs.is_empty() {
        return Err(serde::de::Error::custom(
            "caCertificateRefs must contain at least one reference",
        ));
    }
    if refs.len() > 16 {
        return Err(serde::de::Error::custom(
            "caCertificateRefs must contain at most 16 references",
        ));
    }
    for reference in &refs {
        if reference.kind.is_empty() {
            return Err(serde::de::Error::custom("caCertificateRefs.kind must not be empty"));
        }
        if reference.name.is_empty() {
            return Err(serde::de::Error::custom("caCertificateRefs.name must not be empty"));
        }
        if reference.namespace.as_deref() == Some("") {
            return Err(serde::de::Error::custom(
                "caCertificateRefs.namespace must not be empty when present",
            ));
        }
    }
    Ok(refs)
}

/// Gateway-level TLS configuration (Gateway API v1.5 spec.tls)
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GatewaySpecTLS {
    /// Backend TLS — default client cert for upstream mTLS
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<GatewayBackendTLS>,
    /// Frontend TLS configuration for client certificate validation
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frontend: Option<GatewayFrontendTLS>,
}

/// Gateway-level backend TLS — default client cert for upstream mTLS
#[derive(Deserialize, Serialize, Clone, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GatewayBackendTLS {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_certificate_ref: Option<SecretObjectReference>,
    /// Resolved by Controller, not user-configured
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub resolved_client_certificate: Option<Secret>,
}

/// Gateway-level frontend TLS — client certificate validation config
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GatewayFrontendTLS {
    /// Default validation applied to all HTTPS/TLS listeners unless overridden by perPort
    pub default: GatewayFrontendTLSDefault,
    /// Per-port validation overrides
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(max = MAX_GATEWAY_FRONTEND_PER_PORT))]
    pub per_port: Option<Vec<GatewayFrontendTLSPerPort>>,
}

/// Default frontend TLS validation config
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GatewayFrontendTLSDefault {
    /// Validation configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation: Option<FrontendTLSValidation>,
}

/// Per-port frontend TLS validation override
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GatewayFrontendTLSPerPort {
    /// Port number to override
    #[schemars(range(min = crate::gwapi_types::PORT_MIN, max = crate::gwapi_types::PORT_MAX))]
    pub port: i32,
    /// TLS validation config for this port
    pub tls: GatewayFrontendTLSDefault,
}

/// TLS behavior for a Gateway listener (Gateway API `TLSModeType`).
///
/// Stored as a typed enum so typos are rejected at deserialization rather
/// than silently falling through to the default match arm. Mirrors the
/// upstream CRD `enum: [Terminate, Passthrough]` constraint.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, JsonSchema, PartialEq, Eq, Default)]
pub enum GatewayTlsMode {
    /// TLS session terminates at the gateway. Requires `certificateRefs`.
    #[default]
    Terminate,
    /// TLS session passes through; gateway only inspects the ClientHello.
    Passthrough,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GatewayTLSConfig {
    /// Mode defines the TLS behavior for the TLS session
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<GatewayTlsMode>,

    /// CertificateRefs contains references to Kubernetes objects
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certificate_refs: Option<Vec<SecretObjectReference>>,

    /// Options are implementation-specific TLS options
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<serde_json::Value>,

    /// Complete ordered serving-certificate outcomes filled by the Controller.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub resolved_certificate_refs: Option<Vec<ResolvedServingCertificateReference>>,

    /// Frontend TLS validation for client certificates (Gateway API standard mTLS)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub frontend_validation: Option<FrontendTLSValidation>,

    /// Per-reference frontend CA resolution outcomes filled by the Controller.
    /// The vector is complete and ordered exactly like `caCertificateRefs`, so
    /// the Gateway can enforce all-or-nothing strict validation without losing
    /// the source kind identity.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub resolved_frontend_ca_refs: Option<Vec<ResolvedFrontendCAReference>>,

    /// Controller-computed eligibility for admitting a listener whose status is
    /// not Programmed solely because strict frontend TLS must reject it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub frontend_matcher_eligibility: Option<FrontendMatcherEligibility>,
}

#[derive(Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedFrontendCAReference {
    pub index: u8,
    pub source: FrontendCASourceIdentity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_certificate: Option<ByteString>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<FrontendCAResolutionError>,
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct FrontendCASourceIdentity {
    pub kind: FrontendCASourceKind,
    pub namespace: String,
    pub name: String,
}

#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FrontendCASourceKind {
    Secret,
    ConfigMap,
}

#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrontendCAResolutionError {
    InvalidReference,
    RefNotPermitted,
    NotFound,
    MissingCertificate,
    EmptyCertificate,
    InvalidCertificate,
}

#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrontendMatcherEligibility {
    /// Do not publish this listener into the Gateway TLS matcher. Used for
    /// unrelated admission/programming failures that must not be reinterpreted
    /// as frontend client-auth rejection by the data plane.
    Omit,
    Programmed,
    StrictReject,
    RequireExactEdgionTlsCertificate,
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedServingCertificateReference {
    pub index: u16,
    pub source: ServingCertificateSourceIdentity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<Secret>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ServingCertificateResolutionError>,
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct ServingCertificateSourceIdentity {
    pub group: String,
    pub kind: String,
    pub namespace: String,
    pub name: String,
}

#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServingCertificateResolutionError {
    InvalidReference,
    RefNotPermitted,
    NotFound,
}

pub use crate::resources::common::SecretObjectReference;

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AllowedRoutes {
    /// Namespaces indicates namespaces from which Routes may be attached
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespaces: Option<RouteNamespaces>,

    /// Kinds specifies the Route kinds that are allowed
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kinds: Option<Vec<RouteGroupKind>>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RouteNamespaces {
    /// From indicates where Routes should be selected
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,

    /// Selector must be specified when From is set to "Selector"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<serde_json::Value>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RouteGroupKind {
    /// Group is the group of the Route
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,

    /// Kind is the kind of the Route
    pub kind: String,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GatewayAddress {
    /// Type of the address
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub address_type: Option<String>,

    /// Value of the address
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct GatewayStatus {
    /// Addresses assigned to the Gateway
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub addresses: Option<Vec<GatewayStatusAddress>>,

    /// Conditions describe the current conditions of the Gateway.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conditions: Option<Vec<Condition>>,

    /// Listeners provide status for each unique listener port defined in the Gateway.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listeners: Option<Vec<ListenerStatus>>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GatewayStatusAddress {
    /// Type of the address
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub address_type: Option<String>,

    /// Value of the address
    pub value: String,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListenerStatus {
    /// Name is the name of the Listener that this status corresponds to.
    pub name: String,

    /// SupportedKinds is the list indicating the Kinds supported by this listener.
    pub supported_kinds: Vec<RouteGroupKind>,

    /// AttachedRoutes represents the total number of Routes that have been
    /// successfully attached to this Listener.
    pub attached_routes: i32,

    /// Conditions describe the current conditions of this listener.
    pub conditions: Vec<Condition>,
}

/// Return whether Controller status explicitly excludes a Gateway listener
/// from data-plane publication.
///
/// A Controller-computed current status and matching listener are required.
/// Complete API status never grants data-plane publication authority.
pub fn gateway_listener_is_explicitly_ineligible(gateway: &Gateway, listener_name: &str) -> bool {
    gateway_listener_is_conflicted(gateway, listener_name) || gateway_listener_is_not_programmed(gateway, listener_name)
}

/// Return whether status marks the listener as conflicted.
pub fn gateway_listener_is_conflicted(gateway: &Gateway, listener_name: &str) -> bool {
    gateway
        .spec
        .current_status
        .as_ref()
        .and_then(|status| status.listeners.as_ref())
        .and_then(|listeners| listeners.iter().find(|listener| listener.name == listener_name))
        .is_some_and(|listener| {
            listener
                .conditions
                .iter()
                .any(|condition| condition.type_ == "Conflicted" && condition.status == "True")
        })
}

/// Return whether status explicitly marks the listener Accepted or Programmed
/// false. Missing current status or listener is ineligible.
pub fn gateway_listener_is_not_programmed(gateway: &Gateway, listener_name: &str) -> bool {
    gateway
        .spec
        .current_status
        .as_ref()
        .and_then(|status| status.listeners.as_ref())
        .and_then(|listeners| listeners.iter().find(|listener| listener.name == listener_name))
        .is_none_or(|listener| {
            listener.conditions.iter().any(|condition| {
                (condition.type_ == "Programmed" || condition.type_ == "Accepted") && condition.status == "False"
            })
        })
}

#[cfg(test)]
#[path = "../../tests/unit/resources/gateway_test.rs"]
mod tests;
