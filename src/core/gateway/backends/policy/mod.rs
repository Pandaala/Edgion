pub mod backend_tls;

pub use backend_tls::{
    create_backend_tls_policy_handler, get_global_backend_tls_policy_store, BackendTLSPolicyStore,
    BackendTLSPolicyStoreStats,
};

use crate::core::gateway::backends::port_resolver::resolve_port_name;
use crate::types::resources::BackendTLSPolicy;
use std::sync::Arc;

/// Query BackendTLSPolicy for a given Service, optionally scoped to a specific port.
///
/// When `port` is provided, resolves the port number to a port name via the Service
/// spec, then applies Gateway API sectionName matching:
///   1. Prefer policies whose targetRef.sectionName matches the port name (most specific).
///   2. Fall back to policies with no sectionName (applies to entire Service).
///
/// When `port` is None, returns the highest-priority policy regardless of sectionName.
pub fn query_backend_tls_policy_for_service(
    name: &str,
    namespace: Option<&str>,
    port: Option<i32>,
) -> Option<Arc<BackendTLSPolicy>> {
    let policy_store = get_global_backend_tls_policy_store();
    let policies = policy_store.get_policies_for_target(name, namespace);

    if policies.is_empty() {
        return None;
    }

    let port_name = port.and_then(|p| resolve_port_name(name, namespace, p));

    if let Some(ref pn) = port_name {
        if let Some(exact) = policies.iter().find(|pol| {
            pol.spec
                .target_refs
                .iter()
                .any(|tr| tr.section_name.as_deref() == Some(pn.as_str()))
        }) {
            return Some(exact.clone());
        }
    }

    if let Some(wildcard) = policies
        .iter()
        .find(|pol| pol.spec.target_refs.iter().any(|tr| tr.section_name.is_none()))
    {
        return Some(wildcard.clone());
    }

    policies.into_iter().next()
}
