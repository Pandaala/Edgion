use std::net::{IpAddr, SocketAddr};

use crate::backends::discovery::ServiceSelectorGeneration;
use crate::backends::peer_builder::BackendAppProtocol;
use crate::backends::policy::backend_tls::BackendTLSPolicySnapshot;
use crate::backends::BackendTlsPolicyLookup;
use crate::routes::http::match_unit::CompiledBackend;
use edgion_resources::EdgionErrorCode;

pub struct ValidatedDirectEndpoint {
    pub backend_ref_idx: usize,
    pub addr: SocketAddr,
    pub tls_policy: BackendTlsPolicyLookup,
    pub(crate) app_protocol: Option<BackendAppProtocol>,
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
        let Some(authorization) = backend.permitted_service() else {
            continue;
        };
        let Some(service) = discovery.available(authorization.key()) else {
            continue;
        };
        let Ok(section) = service.resolve_section(authorization.section()) else {
            continue;
        };
        let Some(addr) = section.direct_endpoint_addr(target_ip, target_port) else {
            continue;
        };
        let tls_policy = if inherit_tls {
            policies.lookup(authorization.policy_target(), section.identity.name.as_deref())
        } else {
            BackendTlsPolicyLookup::Absent
        };
        if tls_policy.is_invalid() {
            return Err(EdgionErrorCode::BackendTlsPolicyInvalid);
        }
        return Ok(ValidatedDirectEndpoint {
            backend_ref_idx: backend.slot(),
            addr,
            tls_policy,
            app_protocol: section.prepared_app_protocol,
        });
    }
    Err(EdgionErrorCode::BackendPortResolutionFailed)
}

#[cfg(test)]
#[path = "../../tests/unit/backends/validation_test.rs"]
mod tests;
