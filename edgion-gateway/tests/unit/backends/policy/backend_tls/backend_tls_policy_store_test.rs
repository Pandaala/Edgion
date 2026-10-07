use super::*;
use edgion_resources::resources::backend_tls_policy::{
    BackendTLSPolicyCACertificateRef, BackendTLSPolicySpec, BackendTLSPolicyTargetRef, BackendTLSPolicyValidation,
    SubjectAltName, SubjectAltNameType,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use k8s_openapi::ByteString;

fn policy(
    resource_name: &str,
    target_kind: PolicyTargetKind,
    target_name: &str,
    section: Option<&str>,
    marker: Option<ResolvedPolicyTargetClass>,
) -> Arc<BackendTLSPolicy> {
    let (group, kind) = match target_kind {
        PolicyTargetKind::Service => ("", "Service"),
        PolicyTargetKind::EdgionBackend => ("edgion.io", "EdgionBackend"),
    };
    Arc::new(BackendTLSPolicy {
        metadata: ObjectMeta {
            namespace: Some("tenant".to_string()),
            name: Some(resource_name.to_string()),
            uid: Some(format!("uid-{resource_name}")),
            ..Default::default()
        },
        spec: BackendTLSPolicySpec {
            current_status: None,
            target_refs: vec![BackendTLSPolicyTargetRef {
                group: group.to_string(),
                kind: kind.to_string(),
                name: target_name.to_string(),
                section_name: section.map(str::to_string),
            }],
            resolved_target_class: marker,
            validation: BackendTLSPolicyValidation {
                ca_certificate_refs: None,
                hostname: "backend.example.com".to_string(),
                subject_alt_names: None,
                well_known_ca_certificates: None,
            },
            options: None,
            resolved_ca_certificates: None,
            resolved_client_certificate: None,
            use_system_ca: true,
        },
        status: None,
    })
}

fn target_named(kind: PolicyTargetKind, name: &str) -> PolicyTargetKey {
    PolicyTargetKey {
        target_kind: kind,
        namespace: "tenant".to_string(),
        name: name.to_string(),
    }
}

#[test]
fn unsupported_named_ca_is_not_a_runtime_candidate() {
    let mut candidate = (*policy(
        "named-ca",
        PolicyTargetKind::Service,
        "backend",
        None,
        Some(ResolvedPolicyTargetClass::Service),
    ))
    .clone();
    candidate.spec.validation.well_known_ca_certificates = Some(
        edgion_resources::resources::backend_tls_policy::WellKnownCACertificates::Other(
            "example.com/Custom".to_string(),
        ),
    );
    assert!(!BackendTLSPolicyStore::is_valid_candidate(&candidate));
}

#[test]
fn policy_equivalence_ignores_full_status_but_tracks_current_status() {
    let original = policy(
        "status",
        PolicyTargetKind::Service,
        "backend",
        None,
        Some(ResolvedPolicyTargetClass::Service),
    );
    let mut changed = (*original).clone();
    changed.status = Some(serde_json::from_value(serde_json::json!({"ancestors": []})).unwrap());
    assert!(BackendTLSPolicyStore::policy_resource_equivalent(&original, &changed));
    changed.spec.current_status = changed.status.clone();
    assert!(!BackendTLSPolicyStore::policy_resource_equivalent(&original, &changed));
}

fn target(kind: PolicyTargetKind) -> PolicyTargetKey {
    target_named(kind, "shared")
}

fn effective_name(lookup: BackendTlsPolicyLookup) -> Option<String> {
    lookup.effective().map(|policy| policy.policy.name().to_string())
}

#[test]
fn typed_targets_and_sections_do_not_leak() {
    let store = BackendTLSPolicyStore::new();
    store.replace_all(HashMap::from([
        (
            "tenant/service-grpc".to_string(),
            policy(
                "service-grpc",
                PolicyTargetKind::Service,
                "shared",
                Some("grpc"),
                Some(ResolvedPolicyTargetClass::Service),
            ),
        ),
        (
            "tenant/backend".to_string(),
            policy(
                "backend",
                PolicyTargetKind::EdgionBackend,
                "shared",
                None,
                Some(ResolvedPolicyTargetClass::Ai),
            ),
        ),
    ]));
    let snapshot = store.snapshot();
    assert!(snapshot.lookup(&target(PolicyTargetKind::Service), None).is_absent());
    assert_eq!(
        effective_name(snapshot.lookup(&target(PolicyTargetKind::Service), Some("grpc"))).as_deref(),
        Some("service-grpc")
    );
    assert_eq!(
        effective_name(snapshot.lookup(&target(PolicyTargetKind::EdgionBackend), None)).as_deref(),
        Some("backend")
    );
}

#[test]
fn policy_lookup_does_not_rebuild_target_keys() {
    let store = BackendTLSPolicyStore::new();
    store.replace_all(HashMap::from([(
        "tenant/service-https".to_string(),
        policy(
            "service-https",
            PolicyTargetKind::Service,
            "shared",
            Some("https"),
            Some(ResolvedPolicyTargetClass::Service),
        ),
    )]));
    let snapshot = store.snapshot();
    let target = target(PolicyTargetKind::Service);

    let (selected, allocations) =
        crate::test_allocation_counter::count_allocation_stats(|| snapshot.lookup(&target, Some("https")));

    assert_eq!(effective_name(selected).as_deref(), Some("service-https"));
    assert_eq!(allocations.calls, 0);
    assert_eq!(allocations.requested_bytes, 0);
}

#[test]
fn equivalent_update_retains_generation_arc_and_old_handle() {
    let store = BackendTLSPolicyStore::new();
    let input = policy(
        "policy",
        PolicyTargetKind::Service,
        "shared",
        None,
        Some(ResolvedPolicyTargetClass::Service),
    );
    store.replace_all(HashMap::from([("tenant/policy".to_string(), input)]));
    let first = store.snapshot();
    let first_policy = first
        .lookup(&target(PolicyTargetKind::Service), None)
        .effective()
        .cloned()
        .unwrap();

    store.replace_all(HashMap::from([(
        "tenant/policy".to_string(),
        policy(
            "policy",
            PolicyTargetKind::Service,
            "shared",
            None,
            Some(ResolvedPolicyTargetClass::Service),
        ),
    )]));
    let equivalent = store.snapshot();
    assert_eq!(first.generation(), equivalent.generation());
    assert!(Arc::ptr_eq(
        &first_policy,
        equivalent
            .lookup(&target(PolicyTargetKind::Service), None)
            .effective()
            .unwrap()
    ));

    store.replace_all(HashMap::new());
    assert_eq!(store.snapshot().generation(), first.generation() + 1);
    assert_eq!(first_policy.policy.name(), "policy", "old Arc remains usable");
}

#[test]
fn marker_absent_or_mismatched_is_invalid_not_absent() {
    let store = BackendTLSPolicyStore::new();
    store.replace_all(HashMap::from([
        (
            "tenant/absent".to_string(),
            policy("absent", PolicyTargetKind::Service, "shared", None, None),
        ),
        (
            "tenant/mismatch".to_string(),
            policy(
                "mismatch",
                PolicyTargetKind::Service,
                "shared",
                None,
                Some(ResolvedPolicyTargetClass::Ai),
            ),
        ),
    ]));
    assert!(store
        .snapshot()
        .lookup(&target(PolicyTargetKind::Service), None)
        .is_invalid());
}

#[test]
fn multiple_target_refs_are_invalid_on_every_named_service() {
    let store = BackendTLSPolicyStore::new();
    let mut multi = policy(
        "multi",
        PolicyTargetKind::Service,
        "shared",
        None,
        Some(ResolvedPolicyTargetClass::Service),
    );
    Arc::make_mut(&mut multi)
        .spec
        .target_refs
        .push(BackendTLSPolicyTargetRef {
            group: String::new(),
            kind: "Service".to_string(),
            name: "other".to_string(),
            section_name: None,
        });
    store.replace_all(HashMap::from([("tenant/multi".to_string(), multi)]));

    assert!(store
        .snapshot()
        .lookup(&target(PolicyTargetKind::Service), None)
        .is_invalid());
    assert!(store
        .snapshot()
        .lookup(
            &PolicyTargetKey {
                target_kind: PolicyTargetKind::Service,
                namespace: "tenant".to_string(),
                name: "other".to_string(),
            },
            None
        )
        .is_invalid());
}

#[test]
fn invalid_exact_section_does_not_fall_back_to_whole_target() {
    let store = BackendTLSPolicyStore::new();
    let mut invalid_exact = policy(
        "bad-https",
        PolicyTargetKind::Service,
        "shared",
        Some("https"),
        Some(ResolvedPolicyTargetClass::Service),
    );
    Arc::make_mut(&mut invalid_exact).spec.validation.hostname = "not a hostname".to_string();
    store.replace_all(HashMap::from([
        ("tenant/bad-https".to_string(), invalid_exact),
        (
            "tenant/whole".to_string(),
            policy(
                "whole",
                PolicyTargetKind::Service,
                "shared",
                None,
                Some(ResolvedPolicyTargetClass::Service),
            ),
        ),
    ]));
    let snapshot = store.snapshot();
    assert!(snapshot
        .lookup(&target(PolicyTargetKind::Service), Some("https"))
        .is_invalid());
    assert_eq!(
        effective_name(snapshot.lookup(&target(PolicyTargetKind::Service), Some("grpc"))).as_deref(),
        Some("whole")
    );
}

#[test]
fn valid_exact_section_skips_older_invalid_exact_section() {
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;

    let store = BackendTLSPolicyStore::new();
    let mut older_invalid = policy(
        "older",
        PolicyTargetKind::Service,
        "shared",
        Some("https"),
        Some(ResolvedPolicyTargetClass::Service),
    );
    Arc::make_mut(&mut older_invalid).metadata.creation_timestamp = Some(Time("2026-01-01T00:00:00Z".parse().unwrap()));
    Arc::make_mut(&mut older_invalid).spec.validation.hostname = "bad host".to_string();
    let mut newer_valid = policy(
        "newer",
        PolicyTargetKind::Service,
        "shared",
        Some("https"),
        Some(ResolvedPolicyTargetClass::Service),
    );
    Arc::make_mut(&mut newer_valid).metadata.creation_timestamp = Some(Time("2026-02-01T00:00:00Z".parse().unwrap()));
    store.replace_all(HashMap::from([
        ("tenant/older".to_string(), older_invalid),
        ("tenant/newer".to_string(), newer_valid),
    ]));
    assert_eq!(
        effective_name(
            store
                .snapshot()
                .lookup(&target(PolicyTargetKind::Service), Some("https"))
        )
        .as_deref(),
        Some("newer")
    );
}

#[test]
fn incomplete_custom_ca_is_invalid() {
    let store = BackendTLSPolicyStore::new();
    let mut custom = policy(
        "custom",
        PolicyTargetKind::Service,
        "shared",
        None,
        Some(ResolvedPolicyTargetClass::Service),
    );
    Arc::make_mut(&mut custom).spec.use_system_ca = false;
    Arc::make_mut(&mut custom).spec.validation.ca_certificate_refs = Some(vec![BackendTLSPolicyCACertificateRef {
        group: String::new(),
        kind: "ConfigMap".to_string(),
        name: "ca".to_string(),
    }]);
    store.replace_all(HashMap::from([("tenant/custom".to_string(), custom)]));
    assert!(store
        .snapshot()
        .lookup(&target(PolicyTargetKind::Service), None)
        .is_invalid());
}

#[test]
fn profile_fingerprint_changes_with_ca_bytes_and_sans() {
    let store = BackendTLSPolicyStore::new();
    let mut first = policy(
        "tls",
        PolicyTargetKind::Service,
        "shared",
        None,
        Some(ResolvedPolicyTargetClass::Service),
    );
    Arc::make_mut(&mut first).spec.use_system_ca = false;
    Arc::make_mut(&mut first).spec.resolved_ca_certificates = Some(vec![k8s_openapi::api::core::v1::Secret {
        data: Some(std::collections::BTreeMap::from([(
            CA_CERT.to_string(),
            ByteString(b"ca-a".to_vec()),
        )])),
        ..Default::default()
    }]);
    store.replace_all(HashMap::from([("tenant/tls".to_string(), first)]));
    let first_fp = store
        .snapshot()
        .lookup(&target(PolicyTargetKind::Service), None)
        .effective()
        .unwrap()
        .profile
        .fingerprint;

    let mut second = policy(
        "tls",
        PolicyTargetKind::Service,
        "shared",
        None,
        Some(ResolvedPolicyTargetClass::Service),
    );
    Arc::make_mut(&mut second).spec.use_system_ca = false;
    Arc::make_mut(&mut second).spec.resolved_ca_certificates = Some(vec![k8s_openapi::api::core::v1::Secret {
        data: Some(std::collections::BTreeMap::from([(
            CA_CERT.to_string(),
            ByteString(b"ca-b".to_vec()),
        )])),
        ..Default::default()
    }]);
    store.replace_all(HashMap::from([("tenant/tls".to_string(), second)]));
    let second_fp = store
        .snapshot()
        .lookup(&target(PolicyTargetKind::Service), None)
        .effective()
        .unwrap()
        .profile
        .fingerprint;
    assert_ne!(first_fp, second_fp);

    let mut with_san = policy(
        "tls",
        PolicyTargetKind::Service,
        "shared",
        None,
        Some(ResolvedPolicyTargetClass::Service),
    );
    Arc::make_mut(&mut with_san).spec.use_system_ca = false;
    Arc::make_mut(&mut with_san).spec.resolved_ca_certificates = Some(vec![k8s_openapi::api::core::v1::Secret {
        data: Some(std::collections::BTreeMap::from([(
            CA_CERT.to_string(),
            ByteString(b"ca-b".to_vec()),
        )])),
        ..Default::default()
    }]);
    Arc::make_mut(&mut with_san).spec.validation.subject_alt_names = Some(vec![SubjectAltName {
        san_type: SubjectAltNameType::Hostname,
        hostname: Some("alt.example.com".to_string()),
        uri: None,
    }]);
    store.replace_all(HashMap::from([("tenant/tls".to_string(), with_san)]));
    let san_fp = store
        .snapshot()
        .lookup(&target(PolicyTargetKind::Service), None)
        .effective()
        .unwrap()
        .profile
        .fingerprint;
    assert_ne!(second_fp, san_fp);
}

#[test]
fn profile_fingerprint_is_stable_for_identical_inputs() {
    let store = BackendTLSPolicyStore::new();
    let first = policy(
        "tls",
        PolicyTargetKind::Service,
        "shared",
        None,
        Some(ResolvedPolicyTargetClass::Service),
    );
    store.replace_all(HashMap::from([("tenant/tls".to_string(), Arc::clone(&first))]));
    let first_fp = store
        .snapshot()
        .lookup(&target(PolicyTargetKind::Service), None)
        .effective()
        .unwrap()
        .profile
        .fingerprint;

    let second = policy(
        "tls",
        PolicyTargetKind::Service,
        "shared",
        None,
        Some(ResolvedPolicyTargetClass::Service),
    );
    store.replace_all(HashMap::from([("tenant/tls".to_string(), second)]));
    let second_fp = store
        .snapshot()
        .lookup(&target(PolicyTargetKind::Service), None)
        .effective()
        .unwrap()
        .profile
        .fingerprint;
    assert_eq!(first_fp, second_fp);
    assert_eq!(
        BackendTlsProfileIdentity { fingerprint: first_fp }.group_key(),
        BackendTlsProfileIdentity { fingerprint: second_fp }.group_key()
    );
}

#[test]
fn full_and_partial_publication_produce_the_same_lookup() {
    let full = BackendTLSPolicyStore::new();
    let partial = BackendTLSPolicyStore::new();
    let https = policy(
        "https-a",
        PolicyTargetKind::Service,
        "shared",
        Some("https"),
        Some(ResolvedPolicyTargetClass::Service),
    );
    let whole = policy(
        "whole-a",
        PolicyTargetKind::Service,
        "shared",
        None,
        Some(ResolvedPolicyTargetClass::Service),
    );
    full.replace_all(HashMap::from([
        ("tenant/https-a".to_string(), Arc::clone(&https)),
        ("tenant/whole-a".to_string(), Arc::clone(&whole)),
    ]));
    partial.update(
        HashMap::from([("tenant/https-a".to_string(), https)]),
        HashMap::new(),
        &HashSet::new(),
    );
    partial.update(
        HashMap::from([("tenant/whole-a".to_string(), whole)]),
        HashMap::new(),
        &HashSet::new(),
    );

    let full_snap = full.snapshot();
    let partial_snap = partial.snapshot();
    let target = target(PolicyTargetKind::Service);
    assert_eq!(
        effective_name(full_snap.lookup(&target, Some("https"))).as_deref(),
        effective_name(partial_snap.lookup(&target, Some("https"))).as_deref()
    );
    assert_eq!(
        effective_name(full_snap.lookup(&target, Some("grpc"))).as_deref(),
        effective_name(partial_snap.lookup(&target, Some("grpc"))).as_deref()
    );
    assert_eq!(
        full_snap
            .lookup(&target, Some("https"))
            .effective()
            .unwrap()
            .profile
            .fingerprint,
        partial_snap
            .lookup(&target, Some("https"))
            .effective()
            .unwrap()
            .profile
            .fingerprint
    );

    let moved = policy(
        "https-a",
        PolicyTargetKind::Service,
        "shared",
        Some("grpc"),
        Some(ResolvedPolicyTargetClass::Service),
    );
    partial.update(
        HashMap::new(),
        HashMap::from([("tenant/https-a".to_string(), Arc::clone(&moved))]),
        &HashSet::from(["tenant/whole-a".to_string()]),
    );
    full.replace_all(HashMap::from([("tenant/https-a".to_string(), moved)]));
    let full_snap = full.snapshot();
    let partial_snap = partial.snapshot();
    assert!(full_snap.lookup(&target, Some("https")).is_absent());
    assert!(partial_snap.lookup(&target, Some("https")).is_absent());
    assert_eq!(
        effective_name(full_snap.lookup(&target, Some("grpc"))).as_deref(),
        effective_name(partial_snap.lookup(&target, Some("grpc"))).as_deref()
    );
    assert!(full_snap.lookup(&target, None).is_absent());
    assert!(partial_snap.lookup(&target, None).is_absent());
}

#[test]
fn partial_move_rebuilds_old_and_new_targets_only() {
    let store = BackendTLSPolicyStore::new();
    store.replace_all(HashMap::from([
        (
            "tenant/moving".to_string(),
            policy(
                "moving",
                PolicyTargetKind::Service,
                "old",
                None,
                Some(ResolvedPolicyTargetClass::Service),
            ),
        ),
        (
            "tenant/unrelated".to_string(),
            policy(
                "unrelated",
                PolicyTargetKind::Service,
                "unrelated",
                None,
                Some(ResolvedPolicyTargetClass::Service),
            ),
        ),
    ]));
    let before = store.snapshot();
    let unrelated_target = target_named(PolicyTargetKind::Service, "unrelated");
    let unrelated_before = before.target_buckets(&unrelated_target).unwrap();

    store.update(
        HashMap::new(),
        HashMap::from([(
            "tenant/moving".to_string(),
            policy(
                "moving",
                PolicyTargetKind::Service,
                "new",
                None,
                Some(ResolvedPolicyTargetClass::Service),
            ),
        )]),
        &HashSet::new(),
    );

    let after = store.snapshot();
    assert!(after
        .lookup(&target_named(PolicyTargetKind::Service, "old"), None)
        .is_absent());
    assert_eq!(
        effective_name(after.lookup(&target_named(PolicyTargetKind::Service, "new"), None)).as_deref(),
        Some("moving")
    );
    assert!(Arc::ptr_eq(
        &unrelated_before,
        &after.target_buckets(&unrelated_target).unwrap()
    ));
    assert_eq!(after.generation(), before.generation() + 1);
}

#[test]
fn removing_winner_promotes_next_candidate_locally() {
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;

    let store = BackendTLSPolicyStore::new();
    let mut older = policy(
        "older",
        PolicyTargetKind::Service,
        "shared",
        None,
        Some(ResolvedPolicyTargetClass::Service),
    );
    Arc::make_mut(&mut older).metadata.creation_timestamp = Some(Time("2026-01-01T00:00:00Z".parse().unwrap()));
    let mut newer = policy(
        "newer",
        PolicyTargetKind::Service,
        "shared",
        None,
        Some(ResolvedPolicyTargetClass::Service),
    );
    Arc::make_mut(&mut newer).metadata.creation_timestamp = Some(Time("2026-02-01T00:00:00Z".parse().unwrap()));
    store.replace_all(HashMap::from([
        ("tenant/older".to_string(), older),
        ("tenant/newer".to_string(), newer),
    ]));
    assert_eq!(
        effective_name(store.snapshot().lookup(&target(PolicyTargetKind::Service), None)).as_deref(),
        Some("older")
    );

    store.update(
        HashMap::new(),
        HashMap::new(),
        &HashSet::from(["tenant/older".to_string()]),
    );

    assert_eq!(
        effective_name(store.snapshot().lookup(&target(PolicyTargetKind::Service), None)).as_deref(),
        Some("newer")
    );
}

#[test]
fn equivalent_partial_update_is_suppressed_but_metadata_change_is_published() {
    let store = BackendTLSPolicyStore::new();
    store.replace_all(HashMap::from([(
        "tenant/policy".to_string(),
        policy(
            "policy",
            PolicyTargetKind::Service,
            "shared",
            None,
            Some(ResolvedPolicyTargetClass::Service),
        ),
    )]));
    let first = store.snapshot();
    let first_policy = first.get("tenant/policy").unwrap();
    let first_effective = first
        .lookup(&target(PolicyTargetKind::Service), None)
        .effective()
        .cloned()
        .unwrap();

    store.update(
        HashMap::new(),
        HashMap::from([(
            "tenant/policy".to_string(),
            policy(
                "policy",
                PolicyTargetKind::Service,
                "shared",
                None,
                Some(ResolvedPolicyTargetClass::Service),
            ),
        )]),
        &HashSet::new(),
    );
    let equivalent = store.snapshot();
    assert_eq!(equivalent.generation(), first.generation());
    assert!(Arc::ptr_eq(&first_policy, &equivalent.get("tenant/policy").unwrap()));
    assert!(Arc::ptr_eq(
        &first_effective,
        equivalent
            .lookup(&target(PolicyTargetKind::Service), None)
            .effective()
            .unwrap()
    ));

    let mut metadata_changed = (*first_policy).clone();
    metadata_changed.metadata.labels = Some(std::collections::BTreeMap::from([(
        "revision".to_string(),
        "two".to_string(),
    )]));
    store.update(
        HashMap::new(),
        HashMap::from([("tenant/policy".to_string(), Arc::new(metadata_changed))]),
        &HashSet::new(),
    );
    let changed = store.snapshot();
    assert_eq!(changed.generation(), first.generation() + 1);
    assert_eq!(
        changed
            .get("tenant/policy")
            .unwrap()
            .metadata
            .labels
            .as_ref()
            .and_then(|labels| labels.get("revision"))
            .map(String::as_str),
        Some("two")
    );
}

#[test]
fn one_batch_updates_multiple_targets_in_one_generation() {
    let store = BackendTLSPolicyStore::new();
    let before = store.snapshot().generation();
    store.update(
        HashMap::from([
            (
                "tenant/first".to_string(),
                policy(
                    "first",
                    PolicyTargetKind::Service,
                    "first",
                    None,
                    Some(ResolvedPolicyTargetClass::Service),
                ),
            ),
            (
                "tenant/second".to_string(),
                policy(
                    "second",
                    PolicyTargetKind::Service,
                    "second",
                    None,
                    Some(ResolvedPolicyTargetClass::Service),
                ),
            ),
        ]),
        HashMap::new(),
        &HashSet::new(),
    );
    let after = store.snapshot();
    assert_eq!(after.generation(), before + 1);
    assert!(after
        .lookup(&target_named(PolicyTargetKind::Service, "first"), None)
        .effective()
        .is_some());
    assert!(after
        .lookup(&target_named(PolicyTargetKind::Service, "second"), None)
        .effective()
        .is_some());
}

#[test]
fn defensive_overlapping_batch_keeps_remove_over_update_over_add() {
    let store = BackendTLSPolicyStore::new();
    store.update(
        HashMap::from([(
            "tenant/policy".to_string(),
            policy(
                "policy",
                PolicyTargetKind::Service,
                "add",
                None,
                Some(ResolvedPolicyTargetClass::Service),
            ),
        )]),
        HashMap::from([(
            "tenant/policy".to_string(),
            policy(
                "policy",
                PolicyTargetKind::Service,
                "update",
                None,
                Some(ResolvedPolicyTargetClass::Service),
            ),
        )]),
        &HashSet::new(),
    );
    assert!(store
        .snapshot()
        .lookup(&target_named(PolicyTargetKind::Service, "add"), None)
        .is_absent());
    assert!(store
        .snapshot()
        .lookup(&target_named(PolicyTargetKind::Service, "update"), None)
        .effective()
        .is_some());

    store.update(
        HashMap::from([(
            "tenant/policy".to_string(),
            policy(
                "policy",
                PolicyTargetKind::Service,
                "ignored-add",
                None,
                Some(ResolvedPolicyTargetClass::Service),
            ),
        )]),
        HashMap::from([(
            "tenant/policy".to_string(),
            policy(
                "policy",
                PolicyTargetKind::Service,
                "ignored-update",
                None,
                Some(ResolvedPolicyTargetClass::Service),
            ),
        )]),
        &HashSet::from(["tenant/policy".to_string()]),
    );
    assert!(!store.contains("tenant/policy"));
    assert_eq!(store.stats().reverse_index_targets, 0);
    assert_eq!(store.stats().invalid_index_targets, 0);
}

#[test]
fn partial_validity_and_target_support_transitions_clear_stale_occupancy() {
    let store = BackendTLSPolicyStore::new();
    store.replace_all(HashMap::from([(
        "tenant/policy".to_string(),
        policy("policy", PolicyTargetKind::Service, "shared", None, None),
    )]));
    assert!(store
        .snapshot()
        .lookup(&target(PolicyTargetKind::Service), None)
        .is_invalid());

    store.update(
        HashMap::new(),
        HashMap::from([(
            "tenant/policy".to_string(),
            policy(
                "policy",
                PolicyTargetKind::Service,
                "shared",
                None,
                Some(ResolvedPolicyTargetClass::Service),
            ),
        )]),
        &HashSet::new(),
    );
    assert!(store
        .snapshot()
        .lookup(&target(PolicyTargetKind::Service), None)
        .effective()
        .is_some());

    let mut unsupported = policy(
        "policy",
        PolicyTargetKind::Service,
        "shared",
        None,
        Some(ResolvedPolicyTargetClass::Service),
    );
    Arc::make_mut(&mut unsupported).spec.target_refs[0].group = "unsupported.example".to_string();
    store.update(
        HashMap::new(),
        HashMap::from([("tenant/policy".to_string(), unsupported)]),
        &HashSet::new(),
    );
    assert!(store
        .snapshot()
        .lookup(&target(PolicyTargetKind::Service), None)
        .is_absent());
    assert!(
        store.contains("tenant/policy"),
        "raw diagnostics retain unsupported resources"
    );
    assert_eq!(store.stats().reverse_index_targets, 0);
    assert_eq!(store.stats().invalid_index_targets, 0);
}

#[test]
fn missing_timestamp_loses_and_uid_change_replaces_arc() {
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;

    let store = BackendTLSPolicyStore::new();
    let mut missing = policy(
        "missing",
        PolicyTargetKind::Service,
        "shared",
        None,
        Some(ResolvedPolicyTargetClass::Service),
    );
    Arc::make_mut(&mut missing).metadata.creation_timestamp = None;
    let mut dated = policy(
        "dated",
        PolicyTargetKind::Service,
        "shared",
        None,
        Some(ResolvedPolicyTargetClass::Service),
    );
    Arc::make_mut(&mut dated).metadata.creation_timestamp = Some(Time("2026-01-01T00:00:00Z".parse().unwrap()));
    store.replace_all(HashMap::from([
        ("tenant/missing".to_string(), missing),
        ("tenant/dated".to_string(), dated),
    ]));
    assert_eq!(
        effective_name(store.snapshot().lookup(&target(PolicyTargetKind::Service), None)).as_deref(),
        Some("dated")
    );

    let first = store.snapshot().get("tenant/dated").unwrap();
    let mut recreated = (*first).clone();
    recreated.metadata.uid = Some("replacement-uid".to_string());
    store.replace_all(HashMap::from([("tenant/dated".to_string(), Arc::new(recreated))]));
    let second = store.snapshot().get("tenant/dated").unwrap();
    assert!(!Arc::ptr_eq(&first, &second));
}
