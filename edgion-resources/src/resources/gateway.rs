//! Gateway resource definition
//!
//! Gateway defines a network gateway

use k8s_openapi::api::core::v1::Secret;
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// API group for Gateway
pub const GATEWAY_GROUP: &str = "gateway.networking.k8s.io";

/// Kind for Gateway
pub const GATEWAY_KIND: &str = "Gateway";

use super::common::Condition;

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
    /// GatewayClassName used for this Gateway
    pub gateway_class_name: String,

    /// Listeners associated with this Gateway
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listeners: Option<Vec<Listener>>,

    /// Addresses requested for this Gateway
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub addresses: Option<Vec<GatewayAddress>>,

    /// Gateway-level TLS configuration (Gateway API v1.5)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<GatewaySpecTLS>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Listener {
    /// Name of the Listener
    pub name: String,

    /// Hostname associated with the Listener
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,

    /// Port on which the Listener is listening
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
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FrontendTLSValidation {
    /// CACertificateRefs references Secrets or ConfigMaps containing CA certificate bundles
    /// for validating client certificates. Must contain `ca.crt` key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_certificate_refs: Option<Vec<SecretObjectReference>>,

    /// Validation mode (default: AllowValidOnly).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<FrontendValidationMode>,
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
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
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
#[serde(rename_all = "camelCase")]
pub struct GatewayFrontendTLS {
    /// Default validation applied to all HTTPS/TLS listeners unless overridden by perPort
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<GatewayFrontendTLSDefault>,
    /// Per-port validation overrides
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub per_port: Option<Vec<GatewayFrontendTLSPerPort>>,
}

/// Default frontend TLS validation config
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GatewayFrontendTLSDefault {
    /// Validation configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation: Option<FrontendTLSValidation>,
}

/// Per-port frontend TLS validation override
#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GatewayFrontendTLSPerPort {
    /// Port number to override
    pub port: i32,
    /// TLS validation config for this port
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<GatewayFrontendTLSDefault>,
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

    /// Resolved Secret data (filled by Controller, not user-configured)
    /// Contains the actual TLS certificates referenced by certificate_refs
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub secrets: Option<Vec<Secret>>,

    /// Frontend TLS validation for client certificates (Gateway API standard mTLS)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frontend_validation: Option<FrontendTLSValidation>,

    /// Resolved CA Secret data for frontend validation (filled by Controller, not user-configured)
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::redact::redact_opt::serialize"
    )]
    #[schemars(skip)]
    pub resolved_frontend_ca_secrets: Option<Vec<Secret>>,
}

pub use crate::resources::common::SecretObjectReference;

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AllowedRoutes {
    /// Namespaces indicates namespaces from which Routes may be attached
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespaces: Option<RouteNamespaces>,

    /// Kinds specifies the Route kinds that are allowed
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kinds: Option<Vec<RouteGroupKind>>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RouteNamespaces {
    /// From indicates where Routes should be selected
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,

    /// Selector must be specified when From is set to "Selector"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<serde_json::Value>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
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
            secrets: Some(vec![Secret {
                data: Some(data),
                ..Default::default()
            }]),
            frontend_validation: None,
            resolved_frontend_ca_secrets: None,
        };

        // Under the admin guard, the resolved listener serving cert+key is redacted.
        let redacted = {
            let _g = AdminRedactGuard::new();
            serde_json::to_value(&cfg).unwrap()
        };
        assert_eq!(redacted["secrets"], "[redacted]");
        assert!(
            !redacted.to_string().contains("LISTENER-PRIVATE-KEY"),
            "listener private key leaked under guard"
        );

        // Transport (no guard) must still emit the real Secret objects.
        let plain = serde_json::to_value(&cfg).unwrap();
        assert!(
            plain["secrets"].is_array(),
            "real listener secrets must serialize for transport"
        );
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
        mode: Terminate
        certificateRefs:
          - name: server-cert
        frontendValidation:
          caCertificateRefs:
            - name: client-ca
              kind: Secret
              group: ""
"#;
        let gw: Gateway = serde_yaml::from_str(yaml).unwrap();
        let json = serde_json::to_string_pretty(&gw).unwrap();
        println!("JSON:\n{}", json);
        assert!(
            json.contains("frontendValidation"),
            "frontendValidation missing in JSON: {}",
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
        mode: Terminate
        certificateRefs:
          - name: server-cert
        frontendValidation:
          caCertificateRefs:
            - name: client-ca
              kind: Secret
              group: ""
"#;
        let gw: Gateway = serde_yaml::from_str(yaml).unwrap();
        let listener = &gw.spec.listeners.unwrap()[0];
        let tls = listener.tls.as_ref().unwrap();
        assert!(
            tls.frontend_validation.is_some(),
            "frontendValidation should be present"
        );
        let fv = tls.frontend_validation.as_ref().unwrap();
        let ca_refs = fv.ca_certificate_refs.as_ref().unwrap();
        assert_eq!(ca_refs.len(), 1);
        assert_eq!(ca_refs[0].name, "client-ca");
    }

    #[test]
    fn frontend_validation_mode_accepts_known_values() {
        for (input, expected) in [
            ("AllowValidOnly", FrontendValidationMode::AllowValidOnly),
            ("AllowInsecureFallback", FrontendValidationMode::AllowInsecureFallback),
        ] {
            let yaml = format!("mode: {input}\n");
            let fv: FrontendTLSValidation = serde_yaml::from_str(&yaml).unwrap();
            assert_eq!(fv.mode, Some(expected));
        }
    }

    #[test]
    fn frontend_validation_mode_default_when_absent() {
        let fv: FrontendTLSValidation = serde_yaml::from_str("{}").unwrap();
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
        let err = serde_yaml::from_str::<FrontendTLSValidation>("mode: AllowFoo\n").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("AllowFoo") || msg.contains("variant"),
            "expected variant error, got: {msg}"
        );
    }
}
