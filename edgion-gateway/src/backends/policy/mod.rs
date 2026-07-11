pub mod backend_tls;
pub mod backend_traffic;

pub use backend_tls::{
    create_backend_tls_policy_handler, get_global_backend_tls_policy_store, BackendTLSPolicyStore,
    BackendTLSPolicyStoreStats,
};
pub use backend_traffic::{create_backend_traffic_policy_handler, get_backend_traffic_policy_store, resolve_lb_policy};

use crate::backends::port_resolver::resolve_port_name;
use edgion_resources::resources::BackendTLSPolicy;
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

    select_policy_for_service(&policies, name, port_name.as_deref()).cloned()
}

/// Pick the best-matching policy from the reverse-index candidate list for one Service.
///
/// `candidates` are the policies the reverse index already associated with the queried
/// Service, sorted by Gateway API precedence. Because a policy lands in this Service's
/// bucket only when one of its targetRefs has `policy_ns/name == query_ns/name`, every
/// candidate is guaranteed to share the queried namespace — so isolating the targetRefs
/// that point at this Service only needs a `tr.name == name` check (no namespace rebuild).
///
/// Matching order:
///   1. A targetRef at this Service whose sectionName matches `port_name` (most specific).
///   2. A targetRef at this Service with no sectionName (whole-Service).
///   3. The highest-precedence candidate as a last resort.
///
/// Only targetRefs aimed at the queried Service are considered, so a multi-Service policy
/// cannot leak another Service's sectionName scope into this lookup.
fn select_policy_for_service<'a>(
    candidates: &'a [Arc<BackendTLSPolicy>],
    name: &str,
    port_name: Option<&str>,
) -> Option<&'a Arc<BackendTLSPolicy>> {
    if let Some(pn) = port_name {
        if let Some(exact) = candidates.iter().find(|pol| {
            pol.spec
                .target_refs
                .iter()
                .any(|tr| tr.name == name && tr.section_name.as_deref() == Some(pn))
        }) {
            return Some(exact);
        }
    }

    if let Some(wildcard) = candidates.iter().find(|pol| {
        pol.spec
            .target_refs
            .iter()
            .any(|tr| tr.name == name && tr.section_name.is_none())
    }) {
        return Some(wildcard);
    }

    candidates.first()
}

#[cfg(test)]
mod tests {
    use super::*;
    use edgion_resources::resources::backend_tls_policy::*;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

    /// Build a policy named `name` with the given `(target_service, section_name)` refs.
    fn policy(name: &str, refs: &[(&str, Option<&str>)]) -> Arc<BackendTLSPolicy> {
        Arc::new(BackendTLSPolicy {
            metadata: ObjectMeta {
                name: Some(name.to_string()),
                namespace: Some("default".to_string()),
                ..Default::default()
            },
            spec: BackendTLSPolicySpec {
                target_refs: refs
                    .iter()
                    .map(|(svc, section)| BackendTLSPolicyTargetRef {
                        group: "".to_string(),
                        kind: "Service".to_string(),
                        name: svc.to_string(),
                        section_name: section.map(|s| s.to_string()),
                    })
                    .collect(),
                validation: BackendTLSPolicyValidation {
                    ca_certificate_refs: None,
                    hostname: "backend.example.com".to_string(),
                    well_known_ca_certificates: None,
                    subject_alt_names: None,
                },
                options: None,
                resolved_ca_certificates: None,
                resolved_client_certificate: None,
                use_system_ca: false,
            },
            status: None,
        })
    }

    fn selected_name<'a>(
        candidates: &'a [Arc<BackendTLSPolicy>],
        name: &str,
        port_name: Option<&str>,
    ) -> Option<&'a str> {
        select_policy_for_service(candidates, name, port_name).and_then(|p| p.metadata.name.as_deref())
    }

    // A policy whose only "svc-a" ref is section-scoped must not win the whole-Service
    // (wildcard) slot for "svc-a" just because it carries a no-sectionName ref for "svc-b".
    // The genuine whole-Service policy for "svc-a" should be selected instead.
    #[test]
    fn wildcard_ignores_other_service_ref() {
        let candidates = vec![
            policy("cross", &[("svc-a", Some("https")), ("svc-b", None)]),
            policy("whole-a", &[("svc-a", None)]),
        ];
        assert_eq!(selected_name(&candidates, "svc-a", None), Some("whole-a"));
    }

    // The exact (sectionName == port_name) match must also ignore a ref that points at a
    // different Service: a "svc-b/grpc" ref must not satisfy a "svc-a" port=grpc query.
    #[test]
    fn exact_match_ignores_other_service_ref() {
        let candidates = vec![
            policy("cross", &[("svc-a", Some("https")), ("svc-b", Some("grpc"))]),
            policy("grpc-a", &[("svc-a", Some("grpc"))]),
        ];
        assert_eq!(selected_name(&candidates, "svc-a", Some("grpc")), Some("grpc-a"));
    }

    // Regression guard: a legitimate whole-Service policy still matches any port.
    #[test]
    fn whole_service_policy_matches_any_port() {
        let candidates = vec![policy("whole-a", &[("svc-a", None)])];
        assert_eq!(selected_name(&candidates, "svc-a", Some("https")), Some("whole-a"));
    }

    // Regression guard: an exact port match is preferred over the whole-Service fallback.
    #[test]
    fn exact_port_match_preferred_over_wildcard() {
        let candidates = vec![
            policy("https-a", &[("svc-a", Some("https"))]),
            policy("whole-a", &[("svc-a", None)]),
        ];
        assert_eq!(selected_name(&candidates, "svc-a", Some("https")), Some("https-a"));
    }
}
