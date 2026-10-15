pub(crate) static GLOBAL_TLS_STORE_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

use super::*;
use edgion_resources::resources::backend_tls_policy::*;
use edgion_resources::resources::edgion_backend_traffic_policy::{PolicyTargetKind, ResolvedPolicyTargetClass};
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
            prepared_ancestor_admission: Some(edgion_resources::resources::common::policy_ancestor_admission::PreparedPolicyAncestorAdmission::Unrestricted),
            prepared_precedence: Some(
                edgion_resources::resources::common::policy_precedence::PreparedPolicyPrecedence {
                    creation_timestamp: None,
                },
            ),
            current_status: None,
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
                well_known_ca_certificates: Some(WellKnownCACertificates::System),
                subject_alt_names: None,
            },
            options: None,
            resolved_ca_certificates: None,
            resolved_client_certificate: None,
            use_system_ca: true,
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

#[test]
fn policy_ancestor_capacity_selects_winner_before_admission_without_fallback() {
    use edgion_resources::resources::common::policy_ancestor_admission::PreparedPolicyAncestorAdmission;
    use edgion_resources::resources::common::policy_candidates::{
        select_backend_tls_candidate, PolicyCandidate, PolicySection,
    };
    let mut exact = (*policy("a-exact", &[("svc", Some("https"))])).clone();
    exact.spec.prepared_ancestor_admission =
        Some(PreparedPolicyAncestorAdmission::Scoped(vec!["default/existing".into()]));
    let mut loser = (*policy("z-loser", &[("svc", Some("https"))])).clone();
    loser.spec.prepared_ancestor_admission = Some(PreparedPolicyAncestorAdmission::Scoped(Vec::new()));
    let whole = policy("whole", &[("svc", None)]);
    let store = BackendTLSPolicyStore::new();
    let data = std::collections::HashMap::from([
        ("default/a-exact".into(), Arc::new(exact.clone())),
        ("default/z-loser".into(), Arc::new(loser.clone())),
        ("default/whole".into(), whole.clone()),
    ]);
    store.replace_all(data);
    let target = PolicyTargetKey {
        target_kind: PolicyTargetKind::Service,
        namespace: "default".into(),
        name: "svc".into(),
    };
    let lookup = store.snapshot().lookup(&target, Some("https"));
    assert_eq!(
        lookup.effective().unwrap().policy.metadata.name.as_deref(),
        Some("a-exact")
    );
    assert!(ancestor_admitted(&lookup, None, Some("default/existing")));
    assert!(!ancestor_admitted(&lookup, None, Some("default/new")));
    assert!(!ancestor_admitted(&lookup, None, None));
    assert!(ancestor_admitted(
        &store.snapshot().lookup(&target, Some("other")),
        None,
        Some("default/new")
    ));
    assert!(
        matches!(select_backend_tls_candidate([&exact,&loser,whole.as_ref()], &target, PolicySection::Named("https")),PolicyCandidate::Effective(p) if p.metadata.name.as_deref()==Some("a-exact"))
    );
    let before = store.snapshot();
    exact.spec.prepared_ancestor_admission = Some(PreparedPolicyAncestorAdmission::Scoped(vec![
        "default/existing".into(),
        "default/new".into(),
    ]));
    store.replace_all(std::collections::HashMap::from([(
        "default/a-exact".into(),
        Arc::new(exact),
    )]));
    assert!(store.snapshot().generation() > before.generation());
    assert!(ancestor_admitted(
        &store.snapshot().lookup(&target, Some("https")),
        None,
        Some("default/new")
    ));
    assert!(!ancestor_admitted(&lookup, None, Some("default/new")));
}

#[test]
fn policy_ancestor_hot_check_borrows_bounded_keys_without_allocating() {
    use edgion_resources::resources::common::policy_ancestor_admission::PreparedPolicyAncestorAdmission;
    let mut policy = (*policy("bounded", &[("svc", None)])).clone();
    policy.spec.prepared_ancestor_admission = Some(PreparedPolicyAncestorAdmission::Scoped(
        (0..16).map(|i| format!("default/gateway-{i}")).collect(),
    ));
    let lookup = BackendTlsPolicyLookup::effective_from_policy(Arc::new(policy));
    let (_, allocations) = crate::test_allocation_counter::count_allocations(|| {
        for _ in 0..100 {
            assert!(ancestor_admitted(&lookup, None, Some("default/gateway-15")));
            assert!(!ancestor_admitted(&lookup, None, Some("default/new")));
        }
    });
    assert_eq!(allocations, 0);
}
