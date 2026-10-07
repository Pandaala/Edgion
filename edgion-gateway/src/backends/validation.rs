use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use crate::backends::discovery::{ServiceSectionKey, ServiceSelectorGeneration};
use crate::backends::policy::backend_tls::BackendTLSPolicySnapshot;
use crate::backends::{service_section_key_from_port, BackendTlsPolicyLookup};
use crate::routes::http::match_unit::CompiledBackend;
use edgion_resources::resources::edgion_backend_traffic_policy::{PolicyTargetKey, PolicyTargetKind};
use edgion_resources::{EdgionErrorCode, HTTPBackendRef};

/// Route-owned permission to select an endpoint in one canonical Service section.
/// Constructed before publication, never from request-supplied identity.
#[derive(Clone)]
pub struct DirectEndpointAuthorization {
    service_key: String,
    section: ServiceSectionKey,
    policy_target: PolicyTargetKey,
}

impl DirectEndpointAuthorization {
    pub fn compile(backend: &HTTPBackendRef, route_namespace: &str) -> Option<Self> {
        if backend.ref_denied.is_some() {
            return None;
        }
        let edgion_resources::resources::refs::targets::BackendTarget::Service(identity) =
            backend.validated_target(route_namespace).ok()?
        else {
            return None;
        };
        let section = service_section_key_from_port(backend.port).ok()?;
        let namespace = identity.scope().namespace()?;
        Some(Self {
            service_key: format!("{namespace}/{}", backend.name),
            section,
            policy_target: PolicyTargetKey {
                target_kind: PolicyTargetKind::Service,
                namespace: namespace.to_owned(),
                name: backend.name.clone(),
            },
        })
    }
}

pub struct ValidatedDirectEndpoint {
    pub backend_ref_idx: usize,
    pub addr: SocketAddr,
    pub tls_policy: BackendTlsPolicyLookup,
    pub app_protocol: Option<Arc<str>>,
}

/// Prove the requested socket against one captured discovery generation.
/// Direct overrides deliberately include unready endpoints, but never another
/// Service section. An omitted explicit port must have one unambiguous mapping.
pub fn validate_endpoint_in_route(
    target_ip: IpAddr,
    target_port: Option<u16>,
    backends: &[CompiledBackend],
    discovery: &ServiceSelectorGeneration,
    policies: &BackendTLSPolicySnapshot,
    inherit_tls: bool,
) -> Result<ValidatedDirectEndpoint, EdgionErrorCode> {
    for backend in backends {
        let Some(authorization) = &backend.direct_endpoint else {
            continue;
        };
        let Some(service) = discovery.available(&authorization.service_key) else {
            continue;
        };
        let Ok(section) = service.resolve_section(&authorization.section) else {
            continue;
        };
        let Some(addr) = section.direct_endpoint_addr(target_ip, target_port) else {
            continue;
        };
        let tls_policy = if inherit_tls {
            policies.lookup(&authorization.policy_target, section.identity.name.as_deref())
        } else {
            BackendTlsPolicyLookup::Absent
        };
        if tls_policy.is_invalid() {
            return Err(EdgionErrorCode::BackendTlsPolicyInvalid);
        }
        return Ok(ValidatedDirectEndpoint {
            backend_ref_idx: backend.slot,
            addr,
            tls_policy,
            app_protocol: section.identity.app_protocol.clone(),
        });
    }
    Err(EdgionErrorCode::BackendPortResolutionFailed)
}

#[cfg(test)]
#[path = "../../tests/unit/backends/validation_test.rs"]
mod tests;
