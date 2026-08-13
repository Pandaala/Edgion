pub mod backend_tls;
pub mod backend_traffic;

pub use backend_tls::{
    create_backend_tls_policy_handler, get_global_backend_tls_policy_store, BackendTLSPolicyStore,
    BackendTLSPolicyStoreStats,
};
pub use backend_traffic::{create_backend_traffic_policy_handler, get_backend_traffic_policy_store, resolve_lb_policy};

use crate::backends::port_resolver::resolve_port_name;
use crate::backends::LogicalBackendHandle;
#[cfg(test)]
use edgion_resources::resources::edgion_backend_traffic_policy::ResolvedPolicyTargetClass;
use edgion_resources::resources::edgion_backend_traffic_policy::{PolicyTargetKey, PolicyTargetKind};
use edgion_resources::resources::BackendTLSPolicy;
use std::sync::Arc;

/// Query BackendTLSPolicy from the proof-carrying logical backend selected by
/// the route runtime. Denied and invalid slots cannot yield a policy key.
pub fn query_backend_tls_policy_for_logical_backend(
    logical: &LogicalBackendHandle,
    port: Option<i32>,
) -> Option<Arc<BackendTLSPolicy>> {
    let target = logical.policy_target_key()?;
    if target.target_kind != PolicyTargetKind::Service {
        return None;
    }
    query_backend_tls_policy_for_target(&target, port)
}

/// Typed policy lookup shared by canonical HTTP and gRPC backend proofs.
pub(crate) fn query_backend_tls_policy_for_target(
    target: &PolicyTargetKey,
    port: Option<i32>,
) -> Option<Arc<BackendTLSPolicy>> {
    let port_name = port.and_then(|port| resolve_port_name(&target.name, Some(&target.namespace), port));
    get_global_backend_tls_policy_store()
        .snapshot()
        .get_policy(target, port_name.as_deref())
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
                resolved_target_class: Some(ResolvedPolicyTargetClass::Service),
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

    fn selected_name(candidates: &[Arc<BackendTLSPolicy>], name: &str, port_name: Option<&str>) -> Option<String> {
        let snapshot = BackendTLSPolicyStore::new();
        let data = candidates
            .iter()
            .map(|policy| {
                (
                    format!("default/{}", policy.metadata.name.as_deref().unwrap_or("")),
                    Arc::clone(policy),
                )
            })
            .collect();
        snapshot.replace_all(data);
        snapshot
            .snapshot()
            .get_policy(
                &PolicyTargetKey {
                    target_kind: PolicyTargetKind::Service,
                    namespace: "default".to_string(),
                    name: name.to_string(),
                },
                port_name,
            )
            .and_then(|p| p.metadata.name.clone())
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
        assert_eq!(selected_name(&candidates, "svc-a", None).as_deref(), Some("whole-a"));
    }

    // The exact (sectionName == port_name) match must also ignore a ref that points at a
    // different Service: a "svc-b/grpc" ref must not satisfy a "svc-a" port=grpc query.
    #[test]
    fn exact_match_ignores_other_service_ref() {
        let candidates = vec![
            policy("cross", &[("svc-a", Some("https")), ("svc-b", Some("grpc"))]),
            policy("grpc-a", &[("svc-a", Some("grpc"))]),
        ];
        assert_eq!(
            selected_name(&candidates, "svc-a", Some("grpc")).as_deref(),
            Some("grpc-a")
        );
    }

    // Regression guard: a legitimate whole-Service policy still matches any port.
    #[test]
    fn whole_service_policy_matches_any_port() {
        let candidates = vec![policy("whole-a", &[("svc-a", None)])];
        assert_eq!(
            selected_name(&candidates, "svc-a", Some("https")).as_deref(),
            Some("whole-a")
        );
    }

    // Regression guard: an exact port match is preferred over the whole-Service fallback.
    #[test]
    fn exact_port_match_preferred_over_wildcard() {
        let candidates = vec![
            policy("https-a", &[("svc-a", Some("https"))]),
            policy("whole-a", &[("svc-a", None)]),
        ];
        assert_eq!(
            selected_name(&candidates, "svc-a", Some("https")).as_deref(),
            Some("https-a")
        );
    }
}
