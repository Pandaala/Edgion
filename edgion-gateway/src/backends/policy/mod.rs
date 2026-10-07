pub mod backend_tls;
pub mod backend_traffic;

pub use backend_tls::{
    create_backend_tls_policy_handler, get_global_backend_tls_policy_store, BackendTLSPolicyStore,
    BackendTLSPolicyStoreStats, BackendTlsPolicyLookup, BackendTlsProfileIdentity, EffectiveBackendTlsPolicy,
};
pub use backend_traffic::{create_backend_traffic_policy_handler, get_backend_traffic_policy_store};

use crate::backends::discovery::{get_service_discovery_coordinator, ServicePublishedState};
use crate::backends::policy::backend_tls::BackendTLSPolicySnapshot;
use crate::backends::{runtime::CompiledServiceTarget, LogicalBackendHandle};
use edgion_resources::resources::edgion_backend_traffic_policy::PolicyTargetKey;

/// Query BackendTLSPolicy from the proof-carrying logical backend selected by
/// the route runtime. Denied and invalid slots cannot yield a policy key.
pub fn query_backend_tls_policy_for_logical_backend(logical: &LogicalBackendHandle) -> BackendTlsPolicyLookup {
    if let Some(service) = logical.service_target() {
        query_backend_tls_policy_for_service(service)
    } else {
        logical
            .policy_target_key()
            .map(|target| get_global_backend_tls_policy_store().snapshot().lookup(target, None))
            .unwrap_or(BackendTlsPolicyLookup::Absent)
    }
}

pub(crate) fn query_backend_tls_policy_for_service(service: &CompiledServiceTarget) -> BackendTlsPolicyLookup {
    let snapshot = get_global_backend_tls_policy_store().snapshot();
    let generation = get_service_discovery_coordinator().load_generation();
    match generation.service(service.key()) {
        Some(ServicePublishedState::Available(available)) => match available.resolve_section(service.section()) {
            Ok(section) => {
                let mapping = match section.identity.name.as_deref() {
                    Some(name) => ServiceTlsSectionMapping::Named(name),
                    None => ServiceTlsSectionMapping::NumberOnly,
                };
                lookup_for_service_mapping(&snapshot, service.policy_target(), mapping)
            }
            Err(_) => {
                lookup_for_service_mapping(&snapshot, service.policy_target(), ServiceTlsSectionMapping::Unproven)
            }
        },
        _ => lookup_for_service_mapping(&snapshot, service.policy_target(), ServiceTlsSectionMapping::Unproven),
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

#[cfg(test)]
#[path = "../../../tests/unit/backends/policy/mod_test.rs"]
mod tests;
