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
mod tests {
    use super::*;
    use edgion_resources::resources::backend_tls_policy::*;
    use edgion_resources::resources::edgion_backend_traffic_policy::ResolvedPolicyTargetClass;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use std::sync::Arc;

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

    fn selected_name(
        candidates: &[std::sync::Arc<BackendTLSPolicy>],
        name: &str,
        port_name: Option<&str>,
    ) -> Option<String> {
        let snapshot = BackendTLSPolicyStore::new();
        let data = candidates
            .iter()
            .map(|policy| {
                (
                    format!("default/{}", policy.metadata.name.as_deref().unwrap_or("")),
                    std::sync::Arc::clone(policy),
                )
            })
            .collect();
        snapshot.replace_all(data);
        snapshot
            .snapshot()
            .lookup(
                &PolicyTargetKey {
                    target_kind: PolicyTargetKind::Service,
                    namespace: "default".to_string(),
                    name: name.to_string(),
                },
                port_name,
            )
            .effective()
            .and_then(|policy| policy.policy.metadata.name.clone())
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

    fn store_snapshot(
        candidates: &[Arc<BackendTLSPolicy>],
    ) -> crate::backends::policy::backend_tls::BackendTLSPolicySnapshot {
        let store = BackendTLSPolicyStore::new();
        store.replace_all(
            candidates
                .iter()
                .map(|policy| {
                    (
                        format!("default/{}", policy.metadata.name.as_deref().unwrap_or("")),
                        Arc::clone(policy),
                    )
                })
                .collect(),
        );
        store.snapshot()
    }

    fn service_target(name: &str) -> PolicyTargetKey {
        PolicyTargetKey {
            target_kind: PolicyTargetKind::Service,
            namespace: "default".to_string(),
            name: name.to_string(),
        }
    }

    #[test]
    fn unproven_mapping_with_section_scoped_candidate_is_invalid() {
        let snapshot = store_snapshot(&[policy("https-a", &[("svc-a", Some("https"))])]);
        let target = service_target("svc-a");
        assert!(lookup_for_service_mapping(&snapshot, &target, ServiceTlsSectionMapping::Unproven).is_invalid());
    }

    #[test]
    fn number_only_mapping_ignores_unrelated_named_section() {
        let snapshot = store_snapshot(&[policy("https-a", &[("svc-a", Some("https"))])]);
        let target = service_target("svc-a");
        assert!(lookup_for_service_mapping(&snapshot, &target, ServiceTlsSectionMapping::NumberOnly).is_absent());
    }

    #[test]
    fn named_mapping_uses_exact_section() {
        let snapshot = store_snapshot(&[
            policy("https-a", &[("svc-a", Some("https"))]),
            policy("whole-a", &[("svc-a", None)]),
        ]);
        let target = service_target("svc-a");
        assert_eq!(
            lookup_for_service_mapping(&snapshot, &target, ServiceTlsSectionMapping::Named("https"))
                .effective()
                .and_then(|policy| policy.policy.metadata.name.clone())
                .as_deref(),
            Some("https-a")
        );
    }

    #[test]
    fn number_only_mapping_still_takes_whole_target() {
        let snapshot = store_snapshot(&[policy("whole-a", &[("svc-a", None)])]);
        let target = service_target("svc-a");
        assert_eq!(
            lookup_for_service_mapping(&snapshot, &target, ServiceTlsSectionMapping::NumberOnly)
                .effective()
                .and_then(|policy| policy.policy.metadata.name.clone())
                .as_deref(),
            Some("whole-a")
        );
    }
}
