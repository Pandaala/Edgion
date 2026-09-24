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
#[path = "../../tests/unit/resources/proxy_protocol_test.rs"]
mod tests;
