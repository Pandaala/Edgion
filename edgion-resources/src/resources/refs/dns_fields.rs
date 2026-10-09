//! Finite DNS annotation grammar and actual Service/Gateway field ownership.
use super::{GrantQuery, ObjectIdentity, ReferenceError, ReferenceSource, ResourceScope};
use crate::constants::annotations::edgion::{DNS_RESOLVER, DNS_RESOLVER_REF};
use crate::{EdgionGatewayConfig, Gateway};
use k8s_openapi::api::core::v1::Service;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

/// Explicit namespace/name grammar shared with process DNS configuration.
/// This parsed value alone conveys no resource or Grant authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DnsLinkSysKey<'a> {
    namespace: &'a str,
    name: &'a str,
}
impl<'a> DnsLinkSysKey<'a> {
    pub fn parse(value: &'a str) -> Option<Self> {
        let (namespace, name) = value.trim().split_once('/')?;
        let (namespace, name) = (namespace.trim(), name.trim());
        if namespace.is_empty() || name.is_empty() || name.contains('/') {
            return None;
        }
        Some(Self { namespace, name })
    }
    pub fn namespace(self) -> &'a str {
        self.namespace
    }
    pub fn name(self) -> &'a str {
        self.name
    }
}

/// An immutable loan of the actual annotation owner, never a reconstructed source.
/// ```compile_fail
/// use edgion_resources::resources::refs::dns_fields::DnsResolverOwner;
/// let field = {
///     let owner = k8s_openapi::api::core::v1::Service::default();
///     DnsResolverOwner::Service(&owner)
/// };
/// field.query();
/// ```
#[derive(Clone, Copy)]
pub enum DnsResolverOwner<'a> {
    Service(&'a Service),
    Gateway(&'a Gateway),
}
impl<'a> DnsResolverOwner<'a> {
    fn metadata(self) -> &'a ObjectMeta {
        match self {
            Self::Service(owner) => &owner.metadata,
            Self::Gateway(owner) => &owner.metadata,
        }
    }
    pub fn raw_namespace(self) -> Option<&'a str> {
        self.metadata().namespace.as_deref()
    }
    pub fn name(self) -> Option<&'a str> {
        self.metadata().name.as_deref()
    }

    /// Inline server configuration is independent of external-reference identity.
    pub fn inline_servers(self) -> Option<&'a str> {
        self.metadata()
            .annotations
            .as_ref()?
            .get(DNS_RESOLVER)
            .map(String::as_str)
    }

    /// Identity precedes exemptions, permission, dependencies and target lookup.
    /// Malformed annotations retain their established unavailable/fallback behavior.
    pub fn query(self) -> Result<Option<GrantQuery<'a>>, ReferenceError> {
        let Some(value) = self
            .metadata()
            .annotations
            .as_ref()
            .and_then(|a| a.get(DNS_RESOLVER_REF))
        else {
            return Ok(None);
        };
        let (group, kind) = match self {
            Self::Service(_) => ("", "Service"),
            Self::Gateway(_) => ("gateway.networking.k8s.io", "Gateway"),
        };
        let scope = ResourceScope::Namespaced(self.raw_namespace().unwrap_or("default"));
        ObjectIdentity::checked(group, kind, scope, self.name().unwrap_or_default())?;
        let source = ReferenceSource::checked(group, kind, scope)?;
        Ok(DnsLinkSysKey::parse(value)
            .map(|key| GrantQuery::new(source, ObjectIdentity::parsed_dns_linksys(key.namespace, key.name))))
    }
}

/// The exact administrative DNS field of an actual cluster-scoped EGC.
/// Reference checks do not validate unrelated inline servers or TTL.
/// ```compile_fail
/// use edgion_resources::resources::refs::dns_fields::EgcDnsResolverField;
/// let field = {
///     let owner = edgion_resources::EdgionGatewayConfig::new("config", Default::default());
///     EgcDnsResolverField::new(&owner)
/// };
/// field.target();
/// ```
#[derive(Clone, Copy)]
pub struct EgcDnsResolverField<'a>(&'a EdgionGatewayConfig);
impl<'a> EgcDnsResolverField<'a> {
    pub fn new(owner: &'a EdgionGatewayConfig) -> Self {
        Self(owner)
    }

    pub fn config(self) -> Option<&'a crate::resources::edgion_gateway_config::DnsResolverConfig> {
        self.0.spec.dns_resolver.as_ref()
    }

    pub fn target(self) -> Result<Option<ObjectIdentity<'a>>, ReferenceError> {
        let Some(reference) = self.config().and_then(|config| config.link_sys_ref.as_ref()) else {
            return Ok(None);
        };
        if self.0.metadata.namespace.is_some() {
            return Err(ReferenceError::InvalidNamespace);
        }
        ObjectIdentity::checked(
            "edgion.io",
            "EdgionGatewayConfig",
            ResourceScope::Cluster,
            self.0.metadata.name.as_deref().unwrap_or_default(),
        )?;
        ObjectIdentity::checked(
            "edgion.io",
            "LinkSys",
            ResourceScope::Namespaced(&reference.namespace),
            &reference.name,
        )
        .map(Some)
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/refs/dns_fields_test.rs"]
mod tests;
