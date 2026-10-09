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
                well_known_ca_certificates: Some(
                    edgion_resources::resources::backend_tls_policy::WellKnownCACertificates::System,
                ),
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
    Arc::make_mut(&mut custom).spec.validation.well_known_ca_certificates = None;
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
    Arc::make_mut(&mut first).spec.validation.well_known_ca_certificates = None;
    Arc::make_mut(&mut first).spec.validation.ca_certificate_refs = Some(vec![BackendTLSPolicyCACertificateRef {
        group: String::new(),
        kind: "Secret".into(),
        name: "ca".into(),
    }]);
    Arc::make_mut(&mut first).spec.resolved_ca_certificates = Some(vec![k8s_openapi::api::core::v1::Secret {
        metadata: ObjectMeta {
            name: Some("ca".into()),
            namespace: Some("tenant".into()),
            ..Default::default()
        },
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
    Arc::make_mut(&mut second).spec.validation.well_known_ca_certificates = None;
    Arc::make_mut(&mut second).spec.validation.ca_certificate_refs = Some(vec![BackendTLSPolicyCACertificateRef {
        group: String::new(),
        kind: "Secret".into(),
        name: "ca".into(),
    }]);
    Arc::make_mut(&mut second).spec.resolved_ca_certificates = Some(vec![k8s_openapi::api::core::v1::Secret {
        metadata: ObjectMeta {
            name: Some("ca".into()),
            namespace: Some("tenant".into()),
            ..Default::default()
        },
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
    Arc::make_mut(&mut with_san).spec.validation.well_known_ca_certificates = None;
    Arc::make_mut(&mut with_san).spec.validation.ca_certificate_refs = Some(vec![BackendTLSPolicyCACertificateRef {
        group: String::new(),
        kind: "Secret".into(),
        name: "ca".into(),
    }]);
    Arc::make_mut(&mut with_san).spec.resolved_ca_certificates = Some(vec![k8s_openapi::api::core::v1::Secret {
        metadata: ObjectMeta {
            name: Some("ca".into()),
            namespace: Some("tenant".into()),
            ..Default::default()
        },
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

#[test]
fn bare_owner_targets_default_without_losing_invalid_section_occupancy_or_raw_deletion() {
    for full in [false, true] {
        let store = BackendTLSPolicyStore::new();
        let mut whole = (*policy(
            "bare",
            PolicyTargetKind::Service,
            "backend",
            None,
            Some(ResolvedPolicyTargetClass::Service),
        ))
        .clone();
        whole.metadata.namespace = None;
        let target = PolicyTargetKey {
            target_kind: PolicyTargetKind::Service,
            namespace: "default".into(),
            name: "backend".into(),
        };
        store.replace_all(HashMap::from([("bare".into(), Arc::new(whole.clone()))]));
        let pinned = store.snapshot();
        assert!(matches!(
            pinned.lookup(&target, None),
            BackendTlsPolicyLookup::Effective(_)
        ));
        let mut invalid = whole.clone();
        invalid.metadata.name = Some("invalid".into());
        invalid.spec.target_refs[0].section_name = Some("https".into());
        invalid.spec.target_refs.push(invalid.spec.target_refs[0].clone());
        if full {
            store.replace_all(HashMap::from([
                ("bare".into(), Arc::new(whole.clone())),
                ("invalid".into(), Arc::new(invalid)),
            ]));
        } else {
            store.update(
                HashMap::from([("invalid".into(), Arc::new(invalid))]),
                HashMap::new(),
                &HashSet::new(),
            );
        }
        assert!(matches!(
            store.snapshot().lookup(&target, Some("https")),
            BackendTlsPolicyLookup::Invalid
        ));
        assert!(matches!(
            store.snapshot().lookup(&target, None),
            BackendTlsPolicyLookup::Effective(_)
        ));
        assert!(matches!(
            pinned.lookup(&target, Some("https")),
            BackendTlsPolicyLookup::Effective(_)
        ));
        store.update(HashMap::new(), HashMap::new(), &HashSet::from(["invalid".into()]));
        assert!(matches!(
            store.snapshot().lookup(&target, Some("https")),
            BackendTlsPolicyLookup::Effective(_)
        ));
        whole.spec.target_refs[0].name = "next".into();
        store.update(
            HashMap::new(),
            HashMap::from([("bare".into(), Arc::new(whole.clone()))]),
            &HashSet::new(),
        );
        assert!(matches!(
            store.snapshot().lookup(&target, None),
            BackendTlsPolicyLookup::Absent
        ));
        let next = PolicyTargetKey {
            name: "next".into(),
            ..target.clone()
        };
        assert!(matches!(
            store.snapshot().lookup(&next, None),
            BackendTlsPolicyLookup::Effective(_)
        ));
        whole.metadata.namespace = Some("".into());
        store.update(
            HashMap::new(),
            HashMap::from([("bare".into(), Arc::new(whole))]),
            &HashSet::new(),
        );
        assert!(matches!(
            store.snapshot().lookup(&next, None),
            BackendTlsPolicyLookup::Absent
        ));
        store.update(HashMap::new(), HashMap::new(), &HashSet::from(["bare".into()]));
        assert!(store.get("bare").is_none());
        assert!(matches!(
            pinned.lookup(&target, None),
            BackendTlsPolicyLookup::Effective(_)
        ));
    }
}

fn paired_material_policy() -> Arc<BackendTLSPolicy> {
    use edgion_resources::constants::annotations::edgion::{CONTROLLER_CA_SOURCE, CONTROLLER_CA_SOURCE_CONFIG_MAP};
    let mut owner = policy(
        "paired",
        PolicyTargetKind::Service,
        "shared",
        None,
        Some(ResolvedPolicyTargetClass::Service),
    );
    let spec = &mut Arc::make_mut(&mut owner).spec;
    spec.use_system_ca = false;
    spec.validation.well_known_ca_certificates = None;
    spec.validation.ca_certificate_refs = Some(
        ["Secret", "ConfigMap"]
            .map(|kind| BackendTLSPolicyCACertificateRef {
                group: String::new(),
                kind: kind.into(),
                name: "shared-ca".into(),
            })
            .into(),
    );
    spec.resolved_ca_certificates = Some(
        [false, true]
            .map(|configmap| k8s_openapi::api::core::v1::Secret {
                metadata: ObjectMeta {
                    name: Some("shared-ca".into()),
                    namespace: Some("tenant".into()),
                    annotations: configmap
                        .then(|| [(CONTROLLER_CA_SOURCE.into(), CONTROLLER_CA_SOURCE_CONFIG_MAP.into())].into()),
                    ..Default::default()
                },
                data: Some([(CA_CERT.into(), ByteString(b"ca-a".to_vec()))].into()),
                ..Default::default()
            })
            .into(),
    );
    spec.options = Some([("edgion.io/client-certificate-ref".into(), "client".into())].into());
    spec.resolved_client_certificate = Some(k8s_openapi::api::core::v1::Secret {
        metadata: ObjectMeta {
            name: Some("client".into()),
            namespace: Some("tenant".into()),
            ..Default::default()
        },
        data: Some(
            [
                ("tls.crt".into(), ByteString(b"crt".to_vec())),
                ("tls.key".into(), ByteString(b"key".to_vec())),
            ]
            .into(),
        ),
        ..Default::default()
    });
    owner
}

#[test]
fn cold_backend_tls_publication_rejects_typed_decoys_and_missing_client_identity() {
    let base = paired_material_policy();
    for case in [
        "valid",
        "missing-cm",
        "kind-decoy",
        "ca-name",
        "ca-namespace",
        "client-name",
        "client-namespace",
        "client-missing",
        "client-undeclared",
        "surplus",
        "nine",
        "owner",
        "system-spoof",
    ] {
        let store = BackendTLSPolicyStore::new();
        let mut owner = base.clone();
        let candidate = Arc::make_mut(&mut owner);
        match case {
            "valid" => {}
            "missing-cm" => {
                candidate.spec.resolved_ca_certificates.as_mut().unwrap().pop();
            }
            "kind-decoy" => {
                let decoy = candidate.spec.resolved_ca_certificates.as_ref().unwrap()[0].clone();
                candidate.spec.resolved_ca_certificates.as_mut().unwrap()[1] = decoy;
            }
            "ca-name" => {
                candidate.spec.resolved_ca_certificates.as_mut().unwrap()[0]
                    .metadata
                    .name = Some("decoy".into())
            }
            "ca-namespace" => {
                candidate.spec.resolved_ca_certificates.as_mut().unwrap()[0]
                    .metadata
                    .namespace = None
            }
            "client-name" => {
                candidate
                    .spec
                    .resolved_client_certificate
                    .as_mut()
                    .unwrap()
                    .metadata
                    .name = Some("decoy".into())
            }
            "client-namespace" => {
                candidate
                    .spec
                    .resolved_client_certificate
                    .as_mut()
                    .unwrap()
                    .metadata
                    .namespace = Some("elsewhere".into())
            }
            "client-missing" => candidate.spec.resolved_client_certificate = None,
            "client-undeclared" => candidate.spec.options = None,
            "surplus" => candidate.spec.resolved_ca_certificates = Some(vec![Default::default(); 4096]),
            "nine" => {
                candidate.spec.validation.ca_certificate_refs = Some(vec![
                    BackendTLSPolicyCACertificateRef {
                        group: String::new(),
                        kind: "Secret".into(),
                        name: "ca".into()
                    };
                    9
                ])
            }
            "owner" => candidate.metadata.name = Some("bad/name".into()),
            "system-spoof" => candidate.spec.use_system_ca = true,
            _ => unreachable!(),
        }
        assert_eq!(
            BackendTLSPolicyStore::is_valid_candidate(candidate),
            case == "valid",
            "{case}"
        );
        store.replace_all(HashMap::from([("tenant/paired".into(), owner)]));
        assert_eq!(
            store
                .snapshot()
                .lookup(&target(PolicyTargetKind::Service), None)
                .effective()
                .is_some(),
            case == "valid",
            "{case}"
        );
    }
    let mut system = policy(
        "system",
        PolicyTargetKind::Service,
        "shared",
        None,
        Some(ResolvedPolicyTargetClass::Service),
    );
    let candidate = Arc::make_mut(&mut system);
    candidate.spec.options = base.spec.options.clone();
    candidate.spec.resolved_client_certificate = base.spec.resolved_client_certificate.clone();
    assert!(BackendTLSPolicyStore::is_valid_candidate(candidate));
    candidate
        .spec
        .resolved_client_certificate
        .as_mut()
        .unwrap()
        .metadata
        .name = Some("decoy".into());
    assert!(!BackendTLSPolicyStore::is_valid_candidate(candidate));
}

#[test]
fn cold_backend_tls_invalid_update_repair_and_rotation_keep_captured_generation() {
    let store = BackendTLSPolicyStore::new();
    fn valid_pair(ca: bool) -> (Vec<u8>, Vec<u8>) {
        let key = rcgen::KeyPair::generate().unwrap();
        let mut params = rcgen::CertificateParams::new(vec!["backend.example.com".into()]).unwrap();
        if ca {
            params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        } else {
            params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ClientAuth];
        }
        let certificate = params.self_signed(&key).unwrap().pem().into_bytes();
        let private_key = key.serialize_pem().into_bytes();
        let parsed = pingora_core::tls::x509::X509::from_pem(&certificate).unwrap();
        let parsed_key = pingora_core::tls::pkey::PKey::private_key_from_pem(&private_key).unwrap();
        assert!(parsed.public_key().unwrap().public_eq(&parsed_key));
        (certificate, private_key)
    }
    let (ca_a, _) = valid_pair(true);
    let (ca_b, _) = valid_pair(true);
    let (client_a, key_a) = valid_pair(false);
    let (client_b, key_b) = valid_pair(false);
    let mut base = paired_material_policy();
    let spec = &mut Arc::make_mut(&mut base).spec;
    for material in spec.resolved_ca_certificates.as_mut().unwrap() {
        material
            .data
            .as_mut()
            .unwrap()
            .insert(CA_CERT.into(), ByteString(ca_a.clone()));
    }
    spec.resolved_client_certificate.as_mut().unwrap().data = Some(
        [
            ("tls.crt".into(), ByteString(client_a)),
            ("tls.key".into(), ByteString(key_a)),
        ]
        .into(),
    );
    store.replace_all(HashMap::from([("tenant/paired".into(), base.clone())]));
    let old_snapshot = store.snapshot();
    let lookup = old_snapshot.lookup(&target(PolicyTargetKind::Service), None);
    let old = lookup.effective().unwrap().clone();
    let generation = old_snapshot.generation();
    let old_fp = old.profile.fingerprint;
    store.replace_all(HashMap::from([("tenant/paired".into(), base.clone())]));
    assert_eq!(store.snapshot().generation(), generation);
    let mut invalid = base.clone();
    Arc::make_mut(&mut invalid)
        .spec
        .resolved_ca_certificates
        .as_mut()
        .unwrap()
        .pop();
    store.update(
        HashMap::new(),
        HashMap::from([("tenant/paired".into(), invalid)]),
        &HashSet::new(),
    );
    assert!(store
        .snapshot()
        .lookup(&target(PolicyTargetKind::Service), None)
        .is_invalid());
    assert_eq!(old.profile.fingerprint, old_fp);
    assert_eq!(old.policy.spec.resolved_ca_certificates.as_ref().unwrap().len(), 2);
    let mut repaired = base.clone();
    Arc::make_mut(&mut repaired)
        .spec
        .resolved_ca_certificates
        .as_mut()
        .unwrap()[0]
        .data
        .as_mut()
        .unwrap()
        .insert(CA_CERT.into(), ByteString(ca_b));
    store.update(
        HashMap::new(),
        HashMap::from([("tenant/paired".into(), repaired.clone())]),
        &HashSet::new(),
    );
    let ca_lookup = store.snapshot().lookup(&target(PolicyTargetKind::Service), None);
    let ca_fp = ca_lookup.effective().unwrap().profile.fingerprint;
    assert_ne!(ca_fp, old_fp);
    Arc::make_mut(&mut repaired)
        .spec
        .resolved_client_certificate
        .as_mut()
        .unwrap()
        .data = Some(
        [
            ("tls.crt".into(), ByteString(client_b)),
            ("tls.key".into(), ByteString(key_b)),
        ]
        .into(),
    );
    store.update(
        HashMap::new(),
        HashMap::from([("tenant/paired".into(), repaired)]),
        &HashSet::new(),
    );
    let current = store.snapshot().lookup(&target(PolicyTargetKind::Service), None);
    assert_ne!(current.effective().unwrap().profile.fingerprint, ca_fp);
    assert!(std::sync::Arc::ptr_eq(
        &old.policy,
        &old_snapshot
            .lookup(&target(PolicyTargetKind::Service), None)
            .effective()
            .unwrap()
            .policy
    ));
}

#[test]
fn cold_backend_tls_eight_reference_boundary_and_default_material_are_exact() {
    for count in [8, 9] {
        let mut candidate = paired_material_policy();
        let spec = &mut Arc::make_mut(&mut candidate).spec;
        let declaration = spec.validation.ca_certificate_refs.as_ref().unwrap()[0].clone();
        let material = spec.resolved_ca_certificates.as_ref().unwrap()[0].clone();
        spec.validation.ca_certificate_refs = Some(
            (0..count)
                .map(|i| {
                    let mut r = declaration.clone();
                    r.name = format!("ca-{i}");
                    r
                })
                .collect(),
        );
        spec.resolved_ca_certificates = Some(
            (0..count)
                .map(|i| {
                    let mut s = material.clone();
                    s.metadata.name = Some(format!("ca-{i}"));
                    s
                })
                .collect(),
        );
        let store = BackendTLSPolicyStore::new();
        store.replace_all(HashMap::from([("tenant/paired".into(), candidate)]));
        assert_eq!(
            store
                .snapshot()
                .lookup(&target(PolicyTargetKind::Service), None)
                .effective()
                .is_some(),
            count == 8
        );
    }
    let mut candidate = paired_material_policy();
    let owner = Arc::make_mut(&mut candidate);
    owner.metadata.namespace = None;
    for ca in owner.spec.resolved_ca_certificates.as_mut().unwrap() {
        ca.metadata.namespace = None;
    }
    owner
        .spec
        .resolved_client_certificate
        .as_mut()
        .unwrap()
        .metadata
        .namespace = None;
    let target = PolicyTargetKey {
        target_kind: PolicyTargetKind::Service,
        namespace: "default".into(),
        name: "shared".into(),
    };
    let store = BackendTLSPolicyStore::new();
    store.replace_all(HashMap::from([("paired".into(), candidate.clone())]));
    assert!(store.snapshot().lookup(&target, None).effective().is_some());
    Arc::make_mut(&mut candidate)
        .spec
        .resolved_ca_certificates
        .as_mut()
        .unwrap()[0]
        .metadata
        .namespace = Some(String::new());
    store.replace_all(HashMap::from([("paired".into(), candidate)]));
    assert!(store.snapshot().lookup(&target, None).is_invalid());
}
