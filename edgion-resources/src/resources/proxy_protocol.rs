//! Shared inbound PROXY protocol configuration contract.
//!
//! The Controller resolves the operator-facing annotation into the transport
//! state in this module. The Gateway consumes only the resolved state and never
//! reparses operator JSON.

use crate::resources::common::kubernetes_name::{is_dns_label, is_dns_subdomain};
use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use edgion_algo::matcher::radix_ip::IpCidr;

pub const MAX_INBOUND_PROXY_PROTOCOL_ANNOTATION_BYTES: usize = 16 * 1024;
pub const MAX_PROXY_PROTOCOL_LISTENERS: usize = 64;
pub const MAX_PROXY_PROTOCOL_TRUSTED_CIDRS: usize = 256;
pub const MAX_PROXY_PROTOCOL_CIDR_BYTES: usize = 64;

const MAX_SECTION_NAME_BYTES: usize = 253;
const MAX_KUBERNETES_NAME_BYTES: usize = 253;
const MAX_KUBERNETES_NAMESPACE_BYTES: usize = 63;

/// Strict operator-facing value of `edgion.io/inbound-proxy-protocol`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "source",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum InboundProxyProtocolAnnotation {
    Inline {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        listeners: Option<Vec<String>>,
        #[serde(deserialize_with = "deserialize_strict_proxy_protocol_trust_policy")]
        config: ProxyProtocolTrustPolicy,
    },
    ResourceRef {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        listeners: Option<Vec<String>>,
        resource_ref: ProxyProtocolTrustResourceRef,
    },
}

impl InboundProxyProtocolAnnotation {
    /// Parse the bounded annotation and validate fields that do not require a
    /// Gateway or referenced-resource lookup.
    pub fn parse(raw: &str) -> Result<Self, ProxyProtocolConfigErrorCode> {
        if raw.len() > MAX_INBOUND_PROXY_PROTOCOL_ANNOTATION_BYTES {
            return Err(ProxyProtocolConfigErrorCode::AnnotationTooLarge);
        }

        let annotation: Self = serde_json::from_str(raw).map_err(|error| match error.classify() {
            serde_json::error::Category::Syntax | serde_json::error::Category::Eof => {
                ProxyProtocolConfigErrorCode::MalformedJson
            }
            serde_json::error::Category::Data | serde_json::error::Category::Io => {
                ProxyProtocolConfigErrorCode::InvalidShape
            }
        })?;

        validate_listener_names(annotation.listeners())?;
        if let Self::ResourceRef { resource_ref, .. } = &annotation {
            resource_ref.validate()?;
        }
        Ok(annotation)
    }

    pub fn listeners(&self) -> Option<&[String]> {
        match self {
            Self::Inline { listeners, .. } | Self::ResourceRef { listeners, .. } => listeners.as_deref(),
        }
    }

    pub fn policy(&self) -> Option<&ProxyProtocolTrustPolicy> {
        match self {
            Self::Inline { config, .. } => Some(config),
            Self::ResourceRef { .. } => None,
        }
    }

    pub fn resource_ref(&self) -> Option<&ProxyProtocolTrustResourceRef> {
        match self {
            Self::Inline { .. } => None,
            Self::ResourceRef { resource_ref, .. } => Some(resource_ref),
        }
    }
}

/// Operator-only EdgionConfigData reference. It intentionally excludes the
/// Controller-owned `refDenied` transport field from `EdgionConfigDataRef`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProxyProtocolTrustResourceRef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
}

impl ProxyProtocolTrustResourceRef {
    pub fn validate(&self) -> Result<(), ProxyProtocolConfigErrorCode> {
        if !is_dns_subdomain(&self.name, MAX_KUBERNETES_NAME_BYTES) {
            return Err(ProxyProtocolConfigErrorCode::InvalidReference);
        }
        if self
            .namespace
            .as_deref()
            .is_some_and(|namespace| !is_dns_label(namespace, MAX_KUBERNETES_NAMESPACE_BYTES))
        {
            return Err(ProxyProtocolConfigErrorCode::InvalidReference);
        }
        Ok(())
    }
}

/// Raw trust policy accepted from either the annotation or EdgionConfigData.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "mode", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[schemars(deny_unknown_fields)]
pub enum ProxyProtocolTrustPolicy {
    TrustedSources { trusted_cidrs: Vec<String> },
    Required {},
}

/// Annotation values fail during deserialization on unknown policy fields,
/// while EdgionConfigData must first deserialize so its ingest seam can record
/// the unknown path and publish an `Accepted=False` status.
fn deserialize_strict_proxy_protocol_trust_policy<'de, D>(deserializer: D) -> Result<ProxyProtocolTrustPolicy, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(
        tag = "mode",
        rename_all = "camelCase",
        rename_all_fields = "camelCase",
        deny_unknown_fields
    )]
    enum StrictPolicy {
        TrustedSources { trusted_cidrs: Vec<String> },
        Required {},
    }

    Ok(match StrictPolicy::deserialize(deserializer)? {
        StrictPolicy::TrustedSources { trusted_cidrs } => ProxyProtocolTrustPolicy::TrustedSources { trusted_cidrs },
        StrictPolicy::Required {} => ProxyProtocolTrustPolicy::Required {},
    })
}

impl ProxyProtocolTrustPolicy {
    /// Validate and normalize a complete policy. A single invalid entry rejects
    /// the whole policy; this function never returns a partial CIDR set.
    pub fn canonicalize(&self) -> Result<CanonicalProxyProtocolTrustPolicy, ProxyProtocolConfigErrorCode> {
        match self {
            Self::Required {} => Ok(CanonicalProxyProtocolTrustPolicy::Required {}),
            Self::TrustedSources { trusted_cidrs } => {
                if trusted_cidrs.is_empty() || trusted_cidrs.len() > MAX_PROXY_PROTOCOL_TRUSTED_CIDRS {
                    return Err(ProxyProtocolConfigErrorCode::InvalidPolicy);
                }

                let mut canonical = Vec::with_capacity(trusted_cidrs.len());
                let mut seen = HashSet::with_capacity(trusted_cidrs.len());
                for raw in trusted_cidrs {
                    let cidr = canonicalize_cidr(raw)?;
                    if !seen.insert(cidr.clone()) {
                        return Err(ProxyProtocolConfigErrorCode::InvalidPolicy);
                    }
                    canonical.push(cidr);
                }
                canonical.sort_unstable();

                Ok(CanonicalProxyProtocolTrustPolicy::TrustedSources {
                    trusted_cidrs: canonical,
                })
            }
        }
    }
}

/// Validated policy transported to the Gateway.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "mode",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum CanonicalProxyProtocolTrustPolicy {
    TrustedSources { trusted_cidrs: Vec<String> },
    Required {},
}

impl CanonicalProxyProtocolTrustPolicy {
    /// Validate a Controller-produced transport value before a consumer uses it.
    ///
    /// This is deliberately stricter than merely parsing each CIDR: the vector
    /// must already be the exact sorted, deduplicated canonical representation
    /// produced by [`ProxyProtocolTrustPolicy::canonicalize`].
    pub fn validate_transport(&self) -> Result<(), ProxyProtocolConfigErrorCode> {
        match self {
            Self::Required {} => Ok(()),
            Self::TrustedSources { trusted_cidrs } => {
                let canonical = ProxyProtocolTrustPolicy::TrustedSources {
                    trusted_cidrs: trusted_cidrs.clone(),
                }
                .canonicalize()?;
                if &canonical == self {
                    Ok(())
                } else {
                    Err(ProxyProtocolConfigErrorCode::InvalidPolicy)
                }
            }
        }
    }
}

/// Origin of a successfully resolved policy, retained only as bounded
/// diagnostic metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ProxyProtocolPolicySourceKind {
    Inline,
    ResourceRef,
}

/// Bounded machine-readable failure classes transported to the Gateway.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ProxyProtocolConfigErrorCode {
    AnnotationTooLarge,
    MalformedJson,
    InvalidShape,
    InvalidListenerSelection,
    InvalidPolicy,
    InvalidReference,
    ReferenceNotFound,
    ReferenceNotPermitted,
    ReferenceDisabled,
    ReferenceActiveUnsupported,
    ReferenceWrongType,
    ReferenceInvalidPolicy,
    SharedListenerConflict,
    InternalInvariant,
}

/// Validated reference identity safe for bounded startup diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BoundedProxyProtocolReferenceIdentity {
    pub namespace: String,
    pub name: String,
}

impl BoundedProxyProtocolReferenceIdentity {
    pub fn new(namespace: String, name: String) -> Result<Self, ProxyProtocolConfigErrorCode> {
        let reference = ProxyProtocolTrustResourceRef {
            name: name.clone(),
            namespace: Some(namespace.clone()),
        };
        reference.validate()?;
        Ok(Self { namespace, name })
    }
}

/// Controller-resolved transport state. Only `Ready { policy: Required }` may
/// later map to Pingora's mandatory `trusted_sources = None` mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(
    tag = "state",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ResolvedInboundProxyProtocol {
    #[default]
    Disabled,
    Ready {
        listener_names: Vec<String>,
        policy: CanonicalProxyProtocolTrustPolicy,
        source: ProxyProtocolPolicySourceKind,
    },
    Invalid {
        code: ProxyProtocolConfigErrorCode,
        listener_names: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reference: Option<BoundedProxyProtocolReferenceIdentity>,
    },
}

fn validate_listener_names(listener_names: Option<&[String]>) -> Result<(), ProxyProtocolConfigErrorCode> {
    let Some(listener_names) = listener_names else {
        return Ok(());
    };
    if listener_names.is_empty() || listener_names.len() > MAX_PROXY_PROTOCOL_LISTENERS {
        return Err(ProxyProtocolConfigErrorCode::InvalidListenerSelection);
    }

    let mut seen = HashSet::with_capacity(listener_names.len());
    for name in listener_names {
        if !is_dns_label(name, MAX_SECTION_NAME_BYTES) || !seen.insert(name.as_str()) {
            return Err(ProxyProtocolConfigErrorCode::InvalidListenerSelection);
        }
    }
    Ok(())
}

fn canonicalize_cidr(raw: &str) -> Result<String, ProxyProtocolConfigErrorCode> {
    if raw.len() > MAX_PROXY_PROTOCOL_CIDR_BYTES {
        return Err(ProxyProtocolConfigErrorCode::InvalidPolicy);
    }
    let value = raw.trim();
    if value.is_empty() {
        return Err(ProxyProtocolConfigErrorCode::InvalidPolicy);
    }

    let cidr = if value.contains('/') {
        value.to_string()
    } else {
        let ip = value
            .parse::<IpAddr>()
            .map_err(|_| ProxyProtocolConfigErrorCode::InvalidPolicy)?;
        let prefix = if ip.is_ipv4() { 32 } else { 128 };
        format!("{ip}/{prefix}")
    };

    match IpCidr::parse(&cidr).map_err(|_| ProxyProtocolConfigErrorCode::InvalidPolicy)? {
        IpCidr::V4 { addr, prefix_len } => {
            if prefix_len == 0 {
                return Err(ProxyProtocolConfigErrorCode::InvalidPolicy);
            }
            Ok(format!("{}/{prefix_len}", Ipv4Addr::from(addr)))
        }
        IpCidr::V6 { addr, prefix_len } => {
            if prefix_len == 0 {
                return Err(ProxyProtocolConfigErrorCode::InvalidPolicy);
            }
            Ok(format!("{}/{prefix_len}", Ipv6Addr::from(addr)))
        }
    }
}

#[cfg(test)]
mod tests {
    use schemars::schema_for;
    use serde_json::json;

    use super::*;
    use crate::resources::edgion_config_data::{
        edgion_config_data_spec_schema, ConfigEntry, EdgionConfigData, EdgionConfigDataSpec,
    };
    use crate::resources::gateway::GatewaySpec;

    fn inline_json(config: serde_json::Value) -> String {
        json!({"source": "inline", "config": config}).to_string()
    }

    #[test]
    fn proxy_protocol_annotation_enforces_strict_top_level_union() {
        for value in [
            json!({"config": {"mode": "required"}}),
            json!({"source": "future", "config": {"mode": "required"}}),
            json!({
                "source": "inline",
                "config": {"mode": "required"},
                "resourceRef": {"name": "policy"}
            }),
            json!({
                "source": "resourceRef",
                "resourceRef": {"name": "policy"},
                "config": {"mode": "required"}
            }),
        ] {
            assert_eq!(
                InboundProxyProtocolAnnotation::parse(&value.to_string()),
                Err(ProxyProtocolConfigErrorCode::InvalidShape),
                "unexpectedly accepted {value}"
            );
        }
    }

    #[test]
    fn proxy_protocol_annotation_rejects_unknown_reference_transport_fields() {
        for resource_ref in [
            json!({"name": "policy", "group": "edgion.io"}),
            json!({"name": "policy", "kind": "EdgionConfigData"}),
            json!({"name": "policy", "refDenied": {"reason": "denied"}}),
        ] {
            let value = json!({"source": "resourceRef", "resourceRef": resource_ref});
            assert_eq!(
                InboundProxyProtocolAnnotation::parse(&value.to_string()),
                Err(ProxyProtocolConfigErrorCode::InvalidShape)
            );
        }
    }

    #[test]
    fn proxy_protocol_annotation_distinguishes_malformed_and_oversized_json() {
        assert_eq!(
            InboundProxyProtocolAnnotation::parse("{"),
            Err(ProxyProtocolConfigErrorCode::MalformedJson)
        );
        let oversized = "x".repeat(MAX_INBOUND_PROXY_PROTOCOL_ANNOTATION_BYTES + 1);
        assert_eq!(
            InboundProxyProtocolAnnotation::parse(&oversized),
            Err(ProxyProtocolConfigErrorCode::AnnotationTooLarge)
        );
    }

    #[test]
    fn proxy_protocol_listener_selector_is_bounded_unique_and_valid() {
        for listeners in [json!([]), json!(["http", "http"]), json!(["Uppercase"]), json!([""])] {
            let value = json!({
                "source": "inline",
                "listeners": listeners,
                "config": {"mode": "required"}
            });
            assert_eq!(
                InboundProxyProtocolAnnotation::parse(&value.to_string()),
                Err(ProxyProtocolConfigErrorCode::InvalidListenerSelection)
            );
        }

        let listeners = (0..=MAX_PROXY_PROTOCOL_LISTENERS)
            .map(|index| format!("listener-{index}"))
            .collect::<Vec<_>>();
        let value = json!({
            "source": "inline",
            "listeners": listeners,
            "config": {"mode": "required"}
        });
        assert_eq!(
            InboundProxyProtocolAnnotation::parse(&value.to_string()),
            Err(ProxyProtocolConfigErrorCode::InvalidListenerSelection)
        );
    }

    #[test]
    fn proxy_protocol_reference_names_are_strict_and_bounded() {
        for resource_ref in [
            json!({"name": ""}),
            json!({"name": "UPPER"}),
            json!({"name": "valid", "namespace": "bad.namespace"}),
        ] {
            let value = json!({"source": "resourceRef", "resourceRef": resource_ref});
            assert_eq!(
                InboundProxyProtocolAnnotation::parse(&value.to_string()),
                Err(ProxyProtocolConfigErrorCode::InvalidReference)
            );
        }
    }

    #[test]
    fn proxy_protocol_policy_canonicalizes_bare_and_network_addresses() {
        let policy = ProxyProtocolTrustPolicy::TrustedSources {
            trusted_cidrs: vec![
                " 192.168.1.7 ".to_string(),
                "10.2.3.4/8".to_string(),
                "2001:0db8:0000::1/32".to_string(),
                "2001:db8:1::7".to_string(),
            ],
        };
        assert_eq!(
            policy.canonicalize().unwrap(),
            CanonicalProxyProtocolTrustPolicy::TrustedSources {
                trusted_cidrs: vec![
                    "10.0.0.0/8".to_string(),
                    "192.168.1.7/32".to_string(),
                    "2001:db8:1::7/128".to_string(),
                    "2001:db8::/32".to_string(),
                ]
            }
        );
    }

    #[test]
    fn proxy_protocol_policy_rejects_all_invalid_or_unsafe_sets() {
        let invalid_sets = vec![
            vec![],
            vec!["bad".to_string()],
            vec!["0.0.0.0/00".to_string()],
            vec!["::/0".to_string()],
            vec!["10.0.0.1/8".to_string(), "10.0.0.0/8".to_string()],
            vec!["x".repeat(MAX_PROXY_PROTOCOL_CIDR_BYTES + 1)],
            vec![format!("{}10.0.0.1{}", " ".repeat(30), " ".repeat(30))],
        ];
        for trusted_cidrs in invalid_sets {
            let policy = ProxyProtocolTrustPolicy::TrustedSources { trusted_cidrs };
            assert_eq!(policy.canonicalize(), Err(ProxyProtocolConfigErrorCode::InvalidPolicy));
        }

        let policy = ProxyProtocolTrustPolicy::TrustedSources {
            trusted_cidrs: (0..=MAX_PROXY_PROTOCOL_TRUSTED_CIDRS)
                .map(|index| format!("10.{}.{}.1", index / 256, index % 256))
                .collect(),
        };
        assert_eq!(policy.canonicalize(), Err(ProxyProtocolConfigErrorCode::InvalidPolicy));
    }

    #[test]
    fn proxy_protocol_required_mode_has_explicit_wire_form() {
        let annotation = InboundProxyProtocolAnnotation::parse(&inline_json(json!({
            "mode": "required"
        })))
        .unwrap();
        assert_eq!(
            annotation.policy().unwrap().canonicalize().unwrap(),
            CanonicalProxyProtocolTrustPolicy::Required {}
        );
        assert_eq!(
            serde_json::to_value(CanonicalProxyProtocolTrustPolicy::Required {}).unwrap(),
            json!({"mode": "required"})
        );
    }

    #[test]
    fn canonical_proxy_protocol_transport_rejects_noncanonical_or_unsafe_payloads() {
        assert_eq!(
            CanonicalProxyProtocolTrustPolicy::Required {}.validate_transport(),
            Ok(())
        );
        assert_eq!(
            CanonicalProxyProtocolTrustPolicy::TrustedSources {
                trusted_cidrs: vec!["10.0.0.0/8".to_string(), "192.0.2.1/32".to_string()],
            }
            .validate_transport(),
            Ok(())
        );

        let invalid = vec![
            vec![],
            vec!["0.0.0.0/0".to_string()],
            vec!["not-a-cidr".to_string()],
            vec!["x".repeat(MAX_PROXY_PROTOCOL_CIDR_BYTES + 1)],
            vec!["10.0.0.0/8".to_string(), "10.0.0.0/8".to_string()],
            vec!["192.0.2.0/24".to_string(), "10.0.0.0/8".to_string()],
            vec!["10.1.2.3/8".to_string()],
            vec!["192.0.2.1".to_string()],
            vec!["2001:0db8::/32".to_string()],
        ];
        for trusted_cidrs in invalid {
            assert_eq!(
                CanonicalProxyProtocolTrustPolicy::TrustedSources { trusted_cidrs }.validate_transport(),
                Err(ProxyProtocolConfigErrorCode::InvalidPolicy)
            );
        }

        let over_limit = CanonicalProxyProtocolTrustPolicy::TrustedSources {
            trusted_cidrs: (0..=MAX_PROXY_PROTOCOL_TRUSTED_CIDRS)
                .map(|index| format!("10.{}.{}.1/32", index / 256, index % 256))
                .collect(),
        };
        assert_eq!(
            over_limit.validate_transport(),
            Err(ProxyProtocolConfigErrorCode::InvalidPolicy)
        );
    }

    #[test]
    fn proxy_protocol_resolved_state_defaults_disabled_and_round_trips() {
        assert_eq!(
            ResolvedInboundProxyProtocol::default(),
            ResolvedInboundProxyProtocol::Disabled
        );
        assert_eq!(
            serde_json::to_value(ResolvedInboundProxyProtocol::default()).unwrap(),
            json!({"state": "disabled"})
        );

        let spec: GatewaySpec = serde_json::from_value(json!({
            "gatewayClassName": "edgion",
            "listeners": []
        }))
        .unwrap();
        assert_eq!(
            spec.resolved_inbound_proxy_protocol,
            ResolvedInboundProxyProtocol::Disabled
        );
    }

    #[test]
    fn proxy_protocol_resolved_gateway_field_is_not_in_schemars() {
        let schema = schema_for!(GatewaySpec);
        let properties = schema
            .as_value()
            .get("properties")
            .and_then(serde_json::Value::as_object)
            .expect("GatewaySpec schema must contain top-level properties");
        assert!(!properties.contains_key("resolvedInboundProxyProtocol"));
    }

    #[test]
    fn proxy_protocol_config_data_variant_is_strict_without_changing_existing_variants() {
        let strict = json!({
            "data": {
                "type": "ProxyProtocolTrust",
                "config": {"mode": "required", "typo": true}
            }
        });
        let mut strict_spec: EdgionConfigDataSpec =
            serde_json::from_value(strict.clone()).expect("ECD ingest must retain the object for status reporting");
        strict_spec.unknown_fields = crate::schema_check::unknown_key_paths(&strict, edgion_config_data_spec_schema());
        assert_eq!(strict_spec.unknown_fields, ["data.config.typo"]);
        let strict_resource = EdgionConfigData::new("policy", strict_spec);
        assert_eq!(
            strict_resource.validate_pre_parse(),
            ["ProxyProtocolTrust invalid: policy validation failed"]
        );
        assert_eq!(
            InboundProxyProtocolAnnotation::parse(
                &json!({"source": "inline", "config": {"mode": "required", "typo": true}}).to_string()
            ),
            Err(ProxyProtocolConfigErrorCode::InvalidShape)
        );

        let existing = json!({
            "data": {
                "type": "KeyList",
                "config": {"items": [], "typo": true}
            }
        });
        assert!(serde_json::from_value::<EdgionConfigDataSpec>(existing).is_ok());

        let valid: EdgionConfigDataSpec = serde_json::from_value(json!({
            "data": {
                "type": "ProxyProtocolTrust",
                "config": {"mode": "trustedSources", "trustedCidrs": ["10.1.2.3/8"]}
            }
        }))
        .unwrap();
        let ConfigEntry::ProxyProtocolTrust(policy) = valid.data else {
            panic!("expected ProxyProtocolTrust variant")
        };
        assert_eq!(
            policy.canonicalize().unwrap(),
            CanonicalProxyProtocolTrustPolicy::TrustedSources {
                trusted_cidrs: vec!["10.0.0.0/8".to_string()]
            }
        );
    }
}
