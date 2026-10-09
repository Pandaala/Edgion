//! Field defaults and exact target classification shared across routing consumers.
//!
//! Classification grants no lookup or operation authority. Owning adapters bind
//! validated identities separately before dependency registration and resolution.

/// Default group of a standard backend reference. Explicit values stay exact.
pub fn backend_group(group: Option<&str>) -> &str {
    group.unwrap_or("")
}

/// Default kind of a standard backend reference. Empty is an authored invalid kind.
pub fn backend_kind(kind: Option<&str>) -> &str {
    kind.unwrap_or("Service")
}

/// Exact standard Service target; transport strategies are not resource kinds.
pub fn is_standard_service(group: Option<&str>, kind: Option<&str>) -> bool {
    backend_group(group).is_empty() && backend_kind(kind) == "Service"
}

use super::{ObjectIdentity, ReferenceError, ResourceScope};
use crate::resources::http_route::{BackendObjectReference, HTTPBackendRef, EDGION_API_GROUP, GATEWAY_API_GROUP};
use crate::resources::{GRPCBackendRef, TCPBackendRef, TLSBackendRef, UDPBackendRef};

/// A namespace selector is distinct from an object identity and Grant target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NamespaceRoutesSelector<'a> {
    namespace: &'a str,
    kind: &'static str,
}

impl<'a> NamespaceRoutesSelector<'a> {
    pub fn namespace(self) -> &'a str {
        self.namespace
    }
    pub fn kind(self) -> &'static str {
        self.kind
    }
}

/// Validated concrete target or the explicitly supported delegation selector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendTarget<'a> {
    Service(ObjectIdentity<'a>),
    EdgionBackend(ObjectIdentity<'a>),
    Route(ObjectIdentity<'a>),
    NamespaceRoutes(NamespaceRoutesSelector<'a>),
}

impl<'a> BackendTarget<'a> {
    pub fn object(self) -> Option<ObjectIdentity<'a>> {
        match self {
            Self::Service(identity) | Self::EdgionBackend(identity) | Self::Route(identity) => Some(identity),
            Self::NamespaceRoutes(_) => None,
        }
    }
}

/// Identity for the finite custom managed-HTTP Service target shape.
/// Transport modes remain distinct from the core Service object identity.
pub(crate) fn managed_http_service_target<'a>(
    wire: &'a crate::resources::common::external_http::WebhookServiceRef,
    owner_namespace: &'a str,
) -> Result<ObjectIdentity<'a>, ReferenceError> {
    use crate::resources::common::{is_core_api_group, ServiceBackendKind};
    if !is_core_api_group(wire.group.as_deref().unwrap_or_default())
        || !matches!(
            ServiceBackendKind::from_kind(wire.kind.as_ref()),
            ServiceBackendKind::Service
                | ServiceBackendKind::ServiceClusterIp
                | ServiceBackendKind::ServiceEndpointSlice
        )
    {
        return Err(ReferenceError::UnsupportedTarget);
    }
    if !wire.port.is_some_and(|port| (1..=65535).contains(&port)) {
        return Err(ReferenceError::InvalidPort);
    }
    ObjectIdentity::checked(
        "",
        "Service",
        ResourceScope::Namespaced(wire.namespace.as_deref().unwrap_or(owner_namespace)),
        wire.name.as_deref().unwrap_or_default(),
    )
}

fn service_target<'a>(
    group: Option<&'a str>,
    kind: Option<&'a str>,
    namespace: &'a str,
    name: &'a str,
    port: Option<i32>,
) -> Result<ObjectIdentity<'a>, ReferenceError> {
    if !is_standard_service(group, kind) {
        return Err(ReferenceError::UnsupportedTarget);
    }
    if !port.is_some_and(|port| (1..=65535).contains(&port)) {
        return Err(ReferenceError::InvalidPort);
    }
    ObjectIdentity::checked("", "Service", ResourceScope::Namespaced(namespace), name)
}

fn route_target<'a>(
    namespace: &'a str,
    name: &'a str,
    kind: &'static str,
    port: Option<i32>,
) -> Result<BackendTarget<'a>, ReferenceError> {
    if port.is_some() {
        return Err(ReferenceError::InvalidPort);
    }
    if name == "*" {
        // Validate selector namespace without inventing a '*' object identity.
        super::ReferenceSource::checked(GATEWAY_API_GROUP, kind, ResourceScope::Namespaced(namespace))?;
        return Ok(BackendTarget::NamespaceRoutes(NamespaceRoutesSelector {
            namespace,
            kind,
        }));
    }
    ObjectIdentity::checked(GATEWAY_API_GROUP, kind, ResourceScope::Namespaced(namespace), name)
        .map(BackendTarget::Route)
}

impl HTTPBackendRef {
    /// Cold target validation; this does not authorize a read or mint Grant authority.
    pub fn validated_target<'a>(&'a self, route_namespace: &'a str) -> Result<BackendTarget<'a>, ReferenceError> {
        let namespace = self.namespace.as_deref().unwrap_or(route_namespace);
        match (self.group.as_deref(), self.kind.as_deref()) {
            (Some(EDGION_API_GROUP), Some("EdgionBackend")) => {
                if self.port.is_some() {
                    return Err(ReferenceError::InvalidPort);
                }
                ObjectIdentity::checked(
                    EDGION_API_GROUP,
                    "EdgionBackend",
                    ResourceScope::Namespaced(namespace),
                    &self.name,
                )
                .map(BackendTarget::EdgionBackend)
            }
            (Some(GATEWAY_API_GROUP), Some("HTTPRoute")) => route_target(namespace, &self.name, "HTTPRoute", self.port),
            (group, kind) => service_target(group, kind, namespace, &self.name, self.port).map(BackendTarget::Service),
        }
    }
}

impl GRPCBackendRef {
    /// Cold target validation with the GRPCRoute delegation field contract.
    pub fn validated_target<'a>(&'a self, route_namespace: &'a str) -> Result<BackendTarget<'a>, ReferenceError> {
        let namespace = self.namespace.as_deref().unwrap_or(route_namespace);
        if self.is_route_delegation_ref() {
            route_target(namespace, &self.name, "GRPCRoute", self.port)
        } else {
            service_target(
                self.group.as_deref(),
                self.kind.as_deref(),
                namespace,
                &self.name,
                self.port,
            )
            .map(BackendTarget::Service)
        }
    }
}

macro_rules! service_reference {
    ($($reference:ty),+ $(,)?) => { $(
        impl $reference {
            /// Cold standard Service identity validation, shared with final consumers.
            pub fn validated_target<'a>(&'a self, owner_namespace: &'a str) -> Result<ObjectIdentity<'a>, ReferenceError> {
                service_target(self.group.as_deref(), self.kind.as_deref(), self.namespace.as_deref().unwrap_or(owner_namespace), &self.name, self.port)
            }
        }
    )+ };
}
service_reference!(TCPBackendRef, TLSBackendRef, UDPBackendRef, BackendObjectReference);

use crate::resources::backend_tls_policy::BackendTLSPolicyTargetRef;
use crate::resources::edgion_backend_traffic_policy::{PolicyTargetKey, PolicyTargetKind, PolicyTargetRef};

fn policy_kind(group: &str, kind: &str) -> Result<PolicyTargetKind, ReferenceError> {
    match (group, kind) {
        ("", "Service") => Ok(PolicyTargetKind::Service),
        (EDGION_API_GROUP, "EdgionBackend") => Ok(PolicyTargetKind::EdgionBackend),
        _ => Err(ReferenceError::UnsupportedTarget),
    }
}

fn policy_key(group: &str, kind: &str, namespace: &str, name: &str) -> Result<PolicyTargetKey, String> {
    let target_kind = policy_kind(group, kind).map_err(|error| error.to_string())?;
    ObjectIdentity::checked(group, kind, ResourceScope::Namespaced(namespace), name)
        .map_err(|error| error.to_string())?;
    Ok(PolicyTargetKey {
        target_kind,
        namespace: namespace.to_string(),
        name: name.to_string(),
    })
}

impl BackendTLSPolicyTargetRef {
    pub fn target_kind(&self) -> Result<PolicyTargetKind, String> {
        policy_kind(&self.group, &self.kind).map_err(|error| error.to_string())
    }
    pub fn canonical_key(&self, namespace: &str) -> Result<PolicyTargetKey, String> {
        policy_key(&self.group, &self.kind, namespace, &self.name)
    }
}

impl PolicyTargetRef {
    // The finite custom policy target field permits lowercase core convenience.
    fn canonical_group(&self) -> &str {
        match self.group.as_deref() {
            None | Some("core") => "",
            Some(group) => group,
        }
    }
    pub fn target_kind(&self) -> Result<PolicyTargetKind, String> {
        policy_kind(self.canonical_group(), &self.kind).map_err(|error| error.to_string())
    }
    pub fn canonical_key(&self, namespace: &str) -> Result<PolicyTargetKey, String> {
        policy_key(self.canonical_group(), &self.kind, namespace, &self.name)
    }
}

/// Process-output references have explicit target scope, without Kubernetes ownership or Grant authority.
pub(crate) fn process_output_link_sys_target(
    reference: &crate::output::LinkSysRef,
) -> Result<ObjectIdentity<'_>, ReferenceError> {
    ObjectIdentity::checked(
        EDGION_API_GROUP,
        "LinkSys",
        ResourceScope::Namespaced(&reference.namespace),
        &reference.name,
    )
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/refs/targets_test.rs"]
mod tests;
