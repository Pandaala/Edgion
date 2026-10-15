use super::*;
#[test]
fn policy_ancestor_candidate_admission_does_not_change_tls_winner() {
    let mut policy: BackendTLSPolicy = serde_json::from_value(serde_json::json!({
        "apiVersion":"gateway.networking.k8s.io/v1", "kind":"BackendTLSPolicy",
        "metadata":{"namespace":"team", "name":"policy"},
        "spec":{"targetRefs":[{"group":"", "kind":"Service", "name":"backend"}],
            "validation":{"hostname":"backend.example", "wellKnownCACertificates":"System"}}
    }))
    .unwrap();
    policy.spec.prepared_precedence = Some(super::super::policy_precedence::PreparedPolicyPrecedence {
        creation_timestamp: None,
    });
    policy.spec.resolved_target_class = Some(ResolvedPolicyTargetClass::Service);
    policy.spec.use_system_ca = true;
    policy.spec.prepared_ancestor_admission =
        Some(super::super::policy_ancestor_admission::PreparedPolicyAncestorAdmission::Scoped(Vec::new()));
    let target = prepared_backend_tls_target(&policy).unwrap();
    assert!(matches!(
        select_backend_tls_candidate([&policy], &target, PolicySection::NumberOnly),
        PolicyCandidate::Effective(_)
    ));
    policy.spec.target_refs[0].section_name = Some("https".into());
    policy.spec.resolved_target_class = None;
    assert!(matches!(
        select_backend_tls_candidate([&policy], &target, PolicySection::Named("https")),
        PolicyCandidate::Invalid
    ));
    assert!(matches!(
        select_backend_tls_candidate([&policy], &target, PolicySection::Unproven),
        PolicyCandidate::Invalid
    ));
    assert!(matches!(
        select_backend_tls_candidate([&policy], &target, PolicySection::Named("other")),
        PolicyCandidate::Absent
    ));
}
