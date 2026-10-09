//! Actual Gateway inbound PROXY protocol annotation and reference ownership.

use super::{GrantQuery, ObjectIdentity, ReferenceSource, ResourceScope};
use crate::constants::annotations::edgion::INBOUND_PROXY_PROTOCOL;
use crate::resources::proxy_protocol::{
    InboundProxyProtocolAnnotation, ProxyProtocolConfigErrorCode, ProxyProtocolTrustPolicy,
};
use crate::Gateway;

/// Parses only the actual owning field, once. Reference queries borrow this
/// parsed field and its Gateway; inline trust remains an independent contract.
/// ```compile_fail
/// use edgion_resources::resources::refs::proxy_protocol_fields::GatewayProxyProtocolField;
/// let field = {
///     let owner: edgion_resources::Gateway = serde_json::from_str(
///         r#"{"metadata":{"name":"gateway"},"spec":{"gatewayClassName":"public"}}"#).unwrap();
///     GatewayProxyProtocolField::parse(&owner).unwrap()
/// };
/// field.map(|field| field.query().is_ok());
/// ```
pub struct GatewayProxyProtocolField<'a> {
    owner: &'a Gateway,
    annotation: InboundProxyProtocolAnnotation,
}

impl<'a> GatewayProxyProtocolField<'a> {
    pub fn parse(owner: &'a Gateway) -> Result<Option<Self>, ProxyProtocolConfigErrorCode> {
        let Some(raw) = owner
            .metadata
            .annotations
            .as_ref()
            .and_then(|values| values.get(INBOUND_PROXY_PROTOCOL))
        else {
            return Ok(None);
        };
        Ok(Some(Self {
            owner,
            annotation: InboundProxyProtocolAnnotation::parse(raw)?,
        }))
    }

    pub fn listeners(&self) -> Option<&[String]> {
        self.annotation.listeners()
    }

    pub fn policy(&self) -> Option<&ProxyProtocolTrustPolicy> {
        self.annotation.policy()
    }

    /// The query cannot outlive the privately parsed annotation.
    /// ```compile_fail
    /// use edgion_resources::resources::refs::proxy_protocol_fields::GatewayProxyProtocolField;
    /// let owner: edgion_resources::Gateway = serde_json::from_str(
    ///     r#"{"metadata":{"name":"gateway","annotations":{"edgion.io/inbound-proxy-protocol":"{\"source\":\"resourceRef\",\"resourceRef\":{\"name\":\"trust\"}}"}},"spec":{"gatewayClassName":"public"}}"#).unwrap();
    /// let query = {
    ///     let field = GatewayProxyProtocolField::parse(&owner).unwrap().unwrap();
    ///     field.query().unwrap().unwrap()
    /// };
    /// query.target();
    /// ```
    pub fn query(&self) -> Result<Option<GrantQuery<'_>>, ProxyProtocolConfigErrorCode> {
        let Some(reference) = self.annotation.resource_ref() else {
            return Ok(None);
        };
        let namespace = self.owner.metadata.namespace.as_deref().unwrap_or("default");
        let scope = ResourceScope::Namespaced(namespace);
        ObjectIdentity::checked(
            "gateway.networking.k8s.io",
            "Gateway",
            scope,
            self.owner.metadata.name.as_deref().unwrap_or_default(),
        )
        .map_err(|_| ProxyProtocolConfigErrorCode::InvalidReference)?;
        let source = ReferenceSource::checked("gateway.networking.k8s.io", "Gateway", scope)
            .map_err(|_| ProxyProtocolConfigErrorCode::InvalidReference)?;
        let target = ObjectIdentity::checked(
            "edgion.io",
            "EdgionConfigData",
            ResourceScope::Namespaced(reference.namespace.as_deref().unwrap_or(namespace)),
            &reference.name,
        )
        .map_err(|_| ProxyProtocolConfigErrorCode::InvalidReference)?;
        Ok(Some(GrantQuery::new(source, target)))
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/refs/proxy_protocol_fields_test.rs"]
mod tests;
