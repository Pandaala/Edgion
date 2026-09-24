pub mod backend_tls;
pub mod backend_traffic;

pub use backend_tls::{
    create_backend_tls_policy_handler, get_global_backend_tls_policy_store, BackendTLSPolicyStore,
    BackendTLSPolicyStoreStats, BackendTlsPolicyLookup, BackendTlsProfileIdentity, EffectiveBackendTlsPolicy,
};
pub use backend_traffic::{create_backend_traffic_policy_handler, get_backend_traffic_policy_store};

use crate::backends::discovery::{get_service_discovery_coordinator, ServicePublishedState, ServiceSectionKey};
use crate::backends::policy::backend_tls::BackendTLSPolicySnapshot;
use crate::backends::LogicalBackendHandle;
use edgion_resources::resources::edgion_backend_traffic_policy::{PolicyTargetKey, PolicyTargetKind};
use std::num::NonZeroU16;

/// Query BackendTLSPolicy from the proof-carrying logical backend selected by
/// the route runtime. Denied and invalid slots cannot yield a policy key.
pub fn query_backend_tls_policy_for_logical_backend(
    logical: &LogicalBackendHandle,
    port: Option<i32>,
) -> BackendTlsPolicyLookup {
    match logical.policy_target_key() {
        Some(target) => query_backend_tls_policy_for_target(&target, port),
        None => BackendTlsPolicyLookup::Absent,
    }
}

#[derive(Clone, Copy)]
enum ServiceTlsSectionMapping<'a> {
    Named(&'a str),
    NumberOnly,
    Unproven,
}

fn lookup_for_service_mapping(
    snapshot: &BackendTLSPolicySnapshot,
    target: &PolicyTargetKey,
    mapping: ServiceTlsSectionMapping<'_>,
) -> BackendTlsPolicyLookup {
    match mapping {
        ServiceTlsSectionMapping::Named(name) => snapshot.lookup(target, Some(name)),
        ServiceTlsSectionMapping::NumberOnly => snapshot.lookup(target, None),
        ServiceTlsSectionMapping::Unproven if snapshot.any_section_scoped(target) => BackendTlsPolicyLookup::Invalid,
        ServiceTlsSectionMapping::Unproven => snapshot.lookup(target, None),
    }
}

/// Typed policy lookup shared by canonical HTTP and gRPC backend proofs.
pub(crate) fn query_backend_tls_policy_for_target(
    target: &PolicyTargetKey,
    port: Option<i32>,
) -> BackendTlsPolicyLookup {
    let snapshot = get_global_backend_tls_policy_store().snapshot();
    if target.target_kind == PolicyTargetKind::EdgionBackend {
        return snapshot.lookup(target, None);
    }

    let generation = get_service_discovery_coordinator().load_generation();
    let service_key = format!("{}/{}", target.namespace, target.name);
    let requested = port.and_then(|port| u16::try_from(port).ok()).and_then(NonZeroU16::new);
    match (requested, generation.service(&service_key)) {
        (Some(number), Some(ServicePublishedState::Available(available))) => {
            match available.resolve_section(&ServiceSectionKey::Number(number)) {
                Ok(section) => match section.identity.name.clone() {
                    Some(name) => {
                        lookup_for_service_mapping(&snapshot, target, ServiceTlsSectionMapping::Named(name.as_ref()))
                    }
                    None => lookup_for_service_mapping(&snapshot, target, ServiceTlsSectionMapping::NumberOnly),
                },
                Err(_) => lookup_for_service_mapping(&snapshot, target, ServiceTlsSectionMapping::Unproven),
            }
        }
        _ => lookup_for_service_mapping(&snapshot, target, ServiceTlsSectionMapping::Unproven),
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/backends/policy/mod_test.rs"]
mod tests;
