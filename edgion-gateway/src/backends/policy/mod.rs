#[cfg(test)]
pub(crate) use tests::GLOBAL_TLS_STORE_TEST_LOCK;

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
    query_backend_tls_policy_for_service_canonical(service, false).0
}

/// The report and lookup consume the same TLS and discovery reads. Ordinary
/// callers retain the canonical wrapper; cache evidence never reloads stores.
pub(crate) fn query_backend_tls_policy_for_service_report(
    service: &CompiledServiceTarget,
) -> (BackendTlsPolicyLookup, crate::response_cache::TlsStamp) {
    query_backend_tls_policy_for_service_canonical(service, true)
}

fn query_backend_tls_policy_for_service_canonical(
    service: &CompiledServiceTarget,
    observe: bool,
) -> (BackendTlsPolicyLookup, crate::response_cache::TlsStamp) {
    let snapshot = get_global_backend_tls_policy_store().snapshot();
    let generation = get_service_discovery_coordinator().load_generation();
    let (lookup, discovery) = match generation.service(service.key()) {
        Some(ServicePublishedState::Available(available)) => match available.resolve_section(service.section()) {
            Ok(section) => {
                let mapping = match section.identity.name.as_deref() {
                    Some(name) => ServiceTlsSectionMapping::Named(name),
                    None => ServiceTlsSectionMapping::NumberOnly,
                };
                (
                    lookup_for_service_mapping(&snapshot, service.policy_target(), mapping),
                    if observe {
                        crate::response_cache::discovery_stamp(service, available, &section)
                    } else {
                        None
                    },
                )
            }
            Err(_) => (
                lookup_for_service_mapping(&snapshot, service.policy_target(), ServiceTlsSectionMapping::Unproven),
                None,
            ),
        },
        _ => (
            lookup_for_service_mapping(&snapshot, service.policy_target(), ServiceTlsSectionMapping::Unproven),
            None,
        ),
    };
    let report = crate::response_cache::TlsStamp {
        generation: snapshot.generation(),
        discovery,
        result: crate::response_cache::tls_result(&lookup),
    };
    (lookup, report)
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

/// Apply the winning policies' attachment constraints without changing arbitration.
pub(crate) fn ancestor_admitted(
    tls: &BackendTlsPolicyLookup,
    traffic: Option<&backend_traffic::EffectiveBackendTrafficPolicy>,
    gateway_key: Option<&str>,
) -> bool {
    use edgion_resources::resources::common::policy_ancestor_admission::policy_ancestor_permitted;
    let tls_allowed = match tls {
        BackendTlsPolicyLookup::Effective(policy) => {
            policy_ancestor_permitted(policy.ancestor_admission.as_ref(), gateway_key)
        }
        BackendTlsPolicyLookup::Absent | BackendTlsPolicyLookup::Invalid => true,
    };
    tls_allowed
        && traffic.is_none_or(|policy| policy_ancestor_permitted(policy.ancestor_admission.as_ref(), gateway_key))
}

/// L4 preserves live discovery policy application; admission samples each winner once.
pub(crate) fn service_ancestor_admitted(service: &CompiledServiceTarget, gateway_key: Option<&str>) -> bool {
    let tls = query_backend_tls_policy_for_service(service);
    let traffic = get_backend_traffic_policy_store()
        .snapshot()
        .get_effective(service.policy_target());
    ancestor_admitted(&tls, traffic.as_deref(), gateway_key)
}

/// Shared terminal L4 gate: refusal never invokes discovery or retries a sibling.
pub(crate) fn resolve_l4_policy_backend<'a, T, E>(
    selected: crate::backends::SelectedL4Backend<'a>,
    gateway_key: Option<&str>,
    resolve: impl FnOnce(&'a CompiledServiceTarget) -> Result<T, E>,
) -> Result<(&'a CompiledServiceTarget, T), crate::backends::L4ResolveError<E>> {
    use crate::backends::{L4BackendRejection, L4ResolveError};
    let target = selected.service().map_err(L4ResolveError::Rejected)?;
    if !service_ancestor_admitted(target, gateway_key) {
        return Err(L4ResolveError::Rejected(L4BackendRejection::Denied));
    }
    selected.with_service(resolve)
}
