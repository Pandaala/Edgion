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

    /// Controller-resolved inbound PROXY protocol policy transported to the
    /// Gateway. This is not an operator-authored Gateway API field.
    #[serde(default)]
    #[schemars(skip)]
    pub resolved_inbound_proxy_protocol: ResolvedInboundProxyProtocol,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub resolved_attachment_proof: Option<super::edgion_tls::GatewayAttachmentProof>,
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
    #[schemars(length(min = 1, max = 8))]
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
    if refs.len() > 8 {
        return Err(serde::de::Error::custom(
            "caCertificateRefs must contain at most eight references",
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
    pub value: String,
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
mod tests {
    use super::*;

    #[test]
    fn gateway_listener_secrets_redacted_under_guard() {
        use crate::redact::AdminRedactGuard;
        use k8s_openapi::api::core::v1::Secret;
        use k8s_openapi::ByteString;
        use std::collections::BTreeMap;

        let mut data = BTreeMap::new();
        data.insert("tls.key".to_string(), ByteString(b"LISTENER-PRIVATE-KEY".to_vec()));
        let cfg = GatewayTLSConfig {
            mode: None,
            certificate_refs: None,
            options: None,
            resolved_certificate_refs: Some(vec![ResolvedServingCertificateReference {
                index: 0,
                source: ServingCertificateSourceIdentity {
                    group: String::new(),
                    kind: "Secret".to_string(),
                    namespace: "default".to_string(),
                    name: "server".to_string(),
                },
                secret: Some(Secret {
                    data: Some(data),
                    ..Default::default()
                }),
                error: None,
            }]),
            frontend_validation: None,
            resolved_frontend_ca_refs: None,
            frontend_matcher_eligibility: None,
        };

        // Under the admin guard, the resolved listener serving cert+key is redacted.
        let redacted = {
            let _g = AdminRedactGuard::new();
            serde_json::to_value(&cfg).unwrap()
        };
        assert_eq!(redacted["resolvedCertificateRefs"], "[redacted]");
        assert!(
            !redacted.to_string().contains("LISTENER-PRIVATE-KEY"),
            "listener private key leaked under guard"
        );

        // Transport (no guard) must still emit the real Secret objects.
        let plain = serde_json::to_value(&cfg).unwrap();
        assert!(
            plain["resolvedCertificateRefs"].is_array(),
            "real listener secrets must serialize for transport"
        );
        let roundtrip: GatewayTLSConfig = serde_json::from_value(plain).unwrap();
        let outcomes = roundtrip.resolved_certificate_refs.unwrap();
        let key = &outcomes[0].secret.as_ref().unwrap().data.as_ref().unwrap()["tls.key"].0;
        assert_eq!(key, b"LISTENER-PRIVATE-KEY");
    }

    #[test]
    fn test_gateway_serialization() {
        let gateway = Gateway {
            metadata: kube::core::ObjectMeta {
                name: Some("test-gateway".to_string()),
                namespace: Some("default".to_string()),
                ..Default::default()
            },
            spec: GatewaySpec {
                current_status: None,
                gateway_class_name: "test-class".to_string(),
                listeners: Some(vec![Listener {
                    name: "http".to_string(),
                    hostname: None,
                    port: 80,
                    protocol: "HTTP".to_string(),
                    tls: None,
                    allowed_routes: None,
                }]),
                addresses: None,
                tls: None,
                resolved_inbound_proxy_protocol: ResolvedInboundProxyProtocol::Disabled,
                resolved_attachment_proof: None,
            },
            status: None,
        };

        let yaml = serde_yaml::to_string(&gateway).unwrap();
        println!("{}", yaml);
        assert!(yaml.contains("gatewayClassName"));
    }

    #[test]
    fn test_frontend_validation_json_roundtrip() {
        let yaml = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: test-fv
  namespace: default
spec:
  gatewayClassName: test-class
  listeners:
    - name: https
      hostname: test.example.com
      port: 443
      protocol: HTTPS
  tls:
    frontend:
      default:
        validation:
          caCertificateRefs:
            - name: client-ca
              kind: Secret
              group: ""
"#;
        let gw: Gateway = serde_yaml::from_str(yaml).unwrap();
        let json = serde_json::to_string_pretty(&gw).unwrap();
        println!("JSON:\n{}", json);
        assert!(
            json.contains("caCertificateRefs"),
            "standard frontend TLS missing in JSON: {}",
            json
        );
    }

    #[test]
    fn test_frontend_validation_deserialization() {
        let yaml = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: test-fv
  namespace: default
spec:
  gatewayClassName: test-class
  listeners:
    - name: https
      port: 443
      protocol: HTTPS
  tls:
    frontend:
      default:
        validation:
          caCertificateRefs:
            - name: client-ca
              kind: Secret
              group: ""
"#;
        let gw: Gateway = serde_yaml::from_str(yaml).unwrap();
        let fv = gw.spec.tls.unwrap().frontend.unwrap().default.validation.unwrap();
        let ca_refs = &fv.ca_certificate_refs;
        assert_eq!(ca_refs.len(), 1);
        assert_eq!(ca_refs[0].name, "client-ca");
    }

    #[test]
    fn frontend_validation_mode_accepts_known_values() {
        for (input, expected) in [
            ("AllowValidOnly", FrontendValidationMode::AllowValidOnly),
            ("AllowInsecureFallback", FrontendValidationMode::AllowInsecureFallback),
        ] {
            let yaml =
                format!("caCertificateRefs:\n  - group: \"\"\n    kind: ConfigMap\n    name: ca\nmode: {input}\n");
            let fv: FrontendTLSValidation = serde_yaml::from_str(&yaml).unwrap();
            assert_eq!(fv.mode, Some(expected));
        }
    }

    #[test]
    fn frontend_validation_mode_default_when_absent() {
        let fv: FrontendTLSValidation =
            serde_yaml::from_str("caCertificateRefs:\n  - group: \"\"\n    kind: ConfigMap\n    name: ca\n").unwrap();
        assert!(fv.mode.is_none());
        // Consumers fall back to AllowValidOnly via Default when None.
        assert_eq!(
            FrontendValidationMode::default(),
            FrontendValidationMode::AllowValidOnly
        );
    }

    #[test]
    fn frontend_validation_mode_rejects_typo() {
        // Previous behavior silently mapped unknown strings to the strict default.
        // After typing the field, unknown values must fail deserialization so the
        // controller surfaces the error instead of silently overriding intent.
        let err = serde_yaml::from_str::<FrontendTLSValidation>(
            "caCertificateRefs:\n  - group: \"\"\n    kind: ConfigMap\n    name: ca\nmode: AllowFoo\n",
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("AllowFoo") || msg.contains("variant"),
            "expected variant error, got: {msg}"
        );
    }

    #[test]
    fn frontend_validation_requires_standard_reference_shape_and_nonempty_set() {
        for yaml in [
            "caCertificateRefs: []\n",
            "caCertificateRefs:\n  - kind: ConfigMap\n    name: ca\n",
            "caCertificateRefs:\n  - group: \"\"\n    name: ca\n",
            "caCertificateRefs:\n  - group: \"\"\n    kind: ConfigMap\n    name: \"\"\n",
        ] {
            assert!(
                serde_yaml::from_str::<FrontendTLSValidation>(yaml).is_err(),
                "accepted: {yaml}"
            );
        }
    }

    #[test]
    fn frontend_wire_shape_requires_default_and_per_port_tls_but_allows_empty_policy() {
        let missing_default = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata: { name: test }
spec:
  gatewayClassName: test
  tls:
    frontend:
      perPort: []
"#;
        assert!(serde_yaml::from_str::<Gateway>(missing_default).is_err());

        let missing_per_port_tls = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata: { name: test }
spec:
  gatewayClassName: test
  tls:
    frontend:
      default: {}
      perPort:
      - port: 443
"#;
        assert!(serde_yaml::from_str::<Gateway>(missing_per_port_tls).is_err());

        let empty_policies = r#"
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata: { name: test }
spec:
  gatewayClassName: test
  tls:
    frontend:
      default: {}
      perPort:
      - port: 443
        tls: {}
"#;
        let gateway = serde_yaml::from_str::<Gateway>(empty_policies).unwrap();
        let frontend = gateway.spec.tls.unwrap().frontend.unwrap();
        assert!(frontend.default.validation.is_none());
        assert!(frontend.per_port.unwrap()[0].tls.validation.is_none());
    }

    #[test]
    fn listener_projection_fields_are_schema_hidden_and_ca_material_is_redacted() {
        use crate::redact::AdminRedactGuard;
        use k8s_openapi::ByteString;
        use schemars::schema_for;
        use std::collections::BTreeMap;

        let schema = serde_json::to_value(schema_for!(GatewayTLSConfig)).unwrap();
        let schema_json = schema.to_string();
        assert!(!schema_json.contains("frontendValidation"));
        assert!(!schema_json.contains("resolvedFrontendCaRefs"));

        let mut data = BTreeMap::new();
        data.insert("ca.crt".to_string(), ByteString(b"PRIVATE-CLIENT-CA".to_vec()));
        let cfg = GatewayTLSConfig {
            mode: None,
            certificate_refs: None,
            options: None,
            resolved_certificate_refs: None,
            frontend_validation: None,
            resolved_frontend_ca_refs: Some(vec![ResolvedFrontendCAReference {
                index: 0,
                source: FrontendCASourceIdentity {
                    kind: FrontendCASourceKind::Secret,
                    namespace: "default".to_string(),
                    name: "client-ca".to_string(),
                },
                ca_certificate: Some(data.remove("ca.crt").unwrap()),
                error: None,
            }]),
            frontend_matcher_eligibility: None,
        };
        let redacted = {
            let _guard = AdminRedactGuard::new();
            serde_json::to_value(&cfg).unwrap()
        };
        assert_eq!(redacted["resolvedFrontendCaRefs"], "[redacted]");
        assert!(!redacted.to_string().contains("PRIVATE-CLIENT-CA"));
    }
}
